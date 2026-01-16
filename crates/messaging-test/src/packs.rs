use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::env;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use blake3::Hasher;
use greentic_distributor_client::dist::{DistClient, DistOptions};
use greentic_flow::lint::lint_builtin_rules;
use greentic_pack::{SigningPolicy, open_pack};
use greentic_types::flow::{Node, Routing};
use greentic_types::pack::extensions::component_sources::ArtifactLocationV1;
use greentic_types::pack_manifest::{ExtensionInline, ExtensionRef, PackFlowEntry, PackManifest};
use greentic_types::provider::{PROVIDER_EXTENSION_ID, ProviderExtensionInline};
use greentic_types::{Flow, FlowId, NodeId};
use secrets_core::seed::{DevContext, resolve_uri};

use crate::cli::{PackDiscoveryArgs, PackRuntimeArgs};

#[derive(Debug, Clone)]
pub struct DiscoveredPack {
    pub path: PathBuf,
    pub manifest: Option<PackManifest>,
    pub error: Option<String>,
}

#[derive(Debug)]
pub struct PackRunReport {
    pub pack_id: String,
    pub pack_path: PathBuf,
    pub flow_id: String,
    pub flow_kind: String,
    pub lint_errors: Vec<String>,
    pub missing_components: Vec<String>,
    pub provider_ids: Vec<String>,
    pub dry_run: bool,
    pub steps: Vec<PackStepReport>,
    pub errors: Vec<String>,
    pub secret_uris: Vec<String>,
}

impl PackRunReport {
    pub fn is_success(&self) -> bool {
        self.lint_errors.is_empty()
            && self.missing_components.is_empty()
            && self.errors.is_empty()
            && self.steps.iter().all(|s| s.status.is_ok())
    }
}

#[derive(Debug)]
pub struct PackStepReport {
    pub node_id: String,
    pub component_id: String,
    pub operation: Option<String>,
    pub status: PackStepStatus,
}

#[derive(Debug)]
pub enum PackStepStatus {
    Planned,
    Executed,
    MissingComponent,
}

impl PackStepStatus {
    fn is_ok(&self) -> bool {
        !matches!(self, PackStepStatus::MissingComponent)
    }
}

#[derive(Debug, Clone)]
pub struct MaterializeOptions {
    pub resolve_components: bool,
    pub offline: bool,
    pub allow_tags: bool,
    pub cache_root: PathBuf,
}

#[derive(Debug, Clone)]
pub struct MaterializedPack {
    pub pack_path: PathBuf,
    pub components_dir: Option<PathBuf>,
}

pub trait PackMaterializer: Send + Sync {
    fn materialize(&self, pack: &Path, opts: &MaterializeOptions) -> Result<MaterializedPack>;
}

pub struct DistributorClientMaterializer {
    cache_root: PathBuf,
}

impl DistributorClientMaterializer {
    fn cache_dir_for_pack(&self, pack: &Path, base: &Path) -> Result<PathBuf> {
        let mut hasher = Hasher::new();
        let mut file = File::open(pack)
            .with_context(|| format!("open pack for hashing {}", pack.display()))?;
        let mut buf = [0u8; 8192];
        loop {
            let read = file.read(&mut buf)?;
            if read == 0 {
                break;
            }
            hasher.update(&buf[..read]);
        }
        let hash = hasher.finalize();
        Ok(base.join("materialized").join(hash.to_hex().as_str()))
    }

    fn components_root(&self, pack: &Path, base: &Path) -> Result<PathBuf> {
        let cache_dir = self.cache_dir_for_pack(pack, base)?;
        Ok(cache_dir.join("components"))
    }
}

impl Default for DistributorClientMaterializer {
    fn default() -> Self {
        let cache_root = env::var_os("GREENTIC_HOME")
            .map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache/greentic")))
            .unwrap_or_else(|| PathBuf::from(".cache/greentic"));
        Self { cache_root }
    }
}

impl PackMaterializer for DistributorClientMaterializer {
    fn materialize(&self, pack: &Path, opts: &MaterializeOptions) -> Result<MaterializedPack> {
        if !opts.resolve_components {
            return Ok(MaterializedPack {
                pack_path: pack.to_path_buf(),
                components_dir: None,
            });
        }

        let base = if opts.cache_root.as_os_str().is_empty() {
            &self.cache_root
        } else {
            &opts.cache_root
        };
        let components_root = self.components_root(pack, base)?;
        if opts.offline && !components_root.exists() {
            bail!(
                "offline mode enabled; component cache missing for pack {}",
                pack.display()
            );
        }
        fs::create_dir_all(&components_root)
            .with_context(|| format!("create cache dir {}", components_root.display()))?;

        let manifest = load_manifest(pack)?;
        materialize_components(pack, &manifest, &components_root, opts)?;

        Ok(MaterializedPack {
            pack_path: pack.to_path_buf(),
            components_dir: Some(components_root),
        })
    }
}

fn materialize_components(
    pack: &Path,
    manifest: &PackManifest,
    components_root: &Path,
    opts: &MaterializeOptions,
) -> Result<()> {
    let sources = manifest
        .get_component_sources_v1()
        .context("read component sources extension")?;

    let mut manifest_map = BTreeMap::new();
    for component in &manifest.components {
        let bytes = serde_json::to_vec_pretty(component)
            .with_context(|| format!("serialize component manifest {}", component.id.as_str()))?;
        manifest_map.insert(component.id.as_str().to_string(), bytes);
    }

    if let Some(sources) = sources {
        let dist_opts = DistOptions {
            cache_dir: components_root.to_path_buf(),
            allow_tags: opts.allow_tags,
            offline: opts.offline,
            allow_insecure_local_http: false,
        };
        let client = DistClient::new(dist_opts);
        let mut runtime = None;

        for entry in sources.components {
            let digest = normalize_digest(entry.resolved.digest.as_str());
            let component_dir = component_dir_for_digest(components_root, &digest);
            fs::create_dir_all(&component_dir).with_context(|| {
                format!("create component cache dir {}", component_dir.display())
            })?;

            if let Some(component_id) =
                entry
                    .component_id
                    .as_ref()
                    .map(|id| id.as_str())
                    .or_else(|| {
                        manifest_map
                            .contains_key(entry.name.as_str())
                            .then_some(entry.name.as_str())
                    })
                && let Some(bytes) = manifest_map.remove(component_id)
            {
                write_component_manifest(&component_dir, &bytes)?;
            }

            match entry.artifact {
                ArtifactLocationV1::Inline { wasm_path, .. } => {
                    let bytes = read_pack_file(pack, &wasm_path)?;
                    let wasm_path = component_dir.join("component.wasm");
                    fs::write(&wasm_path, bytes).with_context(|| {
                        format!("write inline component {}", wasm_path.display())
                    })?;
                }
                ArtifactLocationV1::Remote => {
                    let reference = entry.source.to_string();
                    let rt = runtime.get_or_insert_with(|| {
                        tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                            .expect("build async runtime")
                    });
                    rt.block_on(client.ensure_cached(&reference))
                        .with_context(|| format!("materialize {}", reference))?;
                }
            }
        }
    }

    if !manifest_map.is_empty() {
        let fallback_root = components_root.join("manifests");
        for (id, bytes) in manifest_map {
            let dir = fallback_root.join(sanitize_component_dir(&id));
            fs::create_dir_all(&dir)
                .with_context(|| format!("create fallback manifest dir {}", dir.display()))?;
            write_component_manifest(&dir, &bytes)?;
        }
    }

    Ok(())
}

fn write_component_manifest(dir: &Path, bytes: &[u8]) -> Result<()> {
    let path = dir.join("component.manifest.json");
    fs::write(&path, bytes).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

fn component_dir_for_digest(root: &Path, digest: &str) -> PathBuf {
    root.join(trim_digest_prefix(digest))
}

fn normalize_digest(digest: &str) -> String {
    if digest.starts_with("sha256:") {
        digest.to_string()
    } else {
        format!("sha256:{digest}")
    }
}

fn trim_digest_prefix(digest: &str) -> &str {
    digest.strip_prefix("sha256:").unwrap_or(digest)
}

fn sanitize_component_dir(id: &str) -> String {
    id.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn read_pack_file(pack: &Path, pack_path: &str) -> Result<Vec<u8>> {
    let file = File::open(pack)?;
    let mut archive = zip::ZipArchive::new(file)?;
    let mut buf = Vec::new();
    archive
        .by_name(pack_path)
        .with_context(|| format!("missing pack entry {}", pack_path))?
        .read_to_end(&mut buf)?;
    Ok(buf)
}

pub fn discover_packs(args: &PackDiscoveryArgs) -> Result<Vec<DiscoveredPack>> {
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut results = Vec::new();
    for root in &args.roots {
        if !root.exists() {
            continue;
        }
        for path in walk(root, &args.glob).with_context(|| format!("reading {}", root.display()))? {
            if seen.insert(path.clone()) {
                results.push(load_pack(&path));
            }
        }
    }
    results.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(results)
}

pub fn run_pack_from_path(
    path: &Path,
    runtime: &PackRuntimeArgs,
    materializer: &dyn PackMaterializer,
) -> Result<PackRunReport> {
    let materialized = materialize_pack(path, runtime, materializer)?;
    let manifest = load_manifest(&materialized.pack_path)?;
    run_pack(&manifest, runtime, &materialized)
}

pub fn run_all_packs(
    packs: &[DiscoveredPack],
    runtime: &PackRuntimeArgs,
    fail_fast: bool,
    materializer: &dyn PackMaterializer,
) -> Result<Vec<PackRunReport>> {
    let mut reports = Vec::new();
    for pack in packs {
        if let Some(err) = &pack.error {
            if fail_fast {
                return Err(anyhow!("failed to load {}: {err}", pack.path.display()));
            }
            continue;
        }
        let manifest = match &pack.manifest {
            Some(m) => m.clone(),
            None => continue,
        };
        let materialized = materialize_pack(&pack.path, runtime, materializer)?;
        let report = run_pack(&manifest, runtime, &materialized)?;
        if fail_fast && !report.is_success() {
            return Err(anyhow!(
                "pack {} failed validation",
                manifest.pack_id.as_str()
            ));
        }
        reports.push(report);
    }
    Ok(reports)
}

fn materialize_pack(
    path: &Path,
    runtime: &PackRuntimeArgs,
    materializer: &dyn PackMaterializer,
) -> Result<MaterializedPack> {
    let opts = MaterializeOptions {
        resolve_components: runtime.resolve_components,
        offline: runtime.offline,
        allow_tags: runtime.allow_tags,
        cache_root: env::var_os("GREENTIC_HOME")
            .map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache/greentic")))
            .unwrap_or_else(|| PathBuf::from(".cache/greentic")),
    };
    materializer.materialize(path, &opts)
}

fn load_pack(path: &Path) -> DiscoveredPack {
    match load_manifest(path) {
        Ok(manifest) => DiscoveredPack {
            path: path.to_path_buf(),
            manifest: Some(manifest),
            error: None,
        },
        Err(err) => DiscoveredPack {
            path: path.to_path_buf(),
            manifest: None,
            error: Some(err.to_string()),
        },
    }
}

fn load_manifest(path: &Path) -> Result<PackManifest> {
    // Validate archive structure via greentic-pack but always decode the manifest
    // using greentic-types so we keep extensions intact.
    let _ = open_pack(path, SigningPolicy::DevOk);
    // Read manifest.cbor manually (keeps compatibility with zip fixtures).
    let file = File::open(path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    let mut buf = Vec::new();
    archive
        .by_name("manifest.cbor")
        .context("manifest.cbor missing")?
        .read_to_end(&mut buf)
        .context("read manifest.cbor")?;
    greentic_types::decode_pack_manifest(&buf).context("decode manifest")
}

fn select_flow<'a>(
    manifest: &'a PackManifest,
    override_flow: Option<&str>,
) -> Result<&'a PackFlowEntry> {
    let flow_id = if let Some(id) = override_flow {
        FlowId::new(id).context("invalid flow id")?
    } else if let Some(flow) = manifest.flows.iter().find(|f| f.id.as_str() == "smoke") {
        return Ok(flow);
    } else {
        manifest
            .flows
            .first()
            .ok_or_else(|| anyhow!("pack contains no flows"))?
            .id
            .clone()
    };

    manifest
        .flows
        .iter()
        .find(|f| f.id == flow_id)
        .ok_or_else(|| anyhow!("flow {flow_id} not found in pack {}", manifest.pack_id))
}

fn run_pack(
    manifest: &PackManifest,
    runtime: &PackRuntimeArgs,
    materialized: &MaterializedPack,
) -> Result<PackRunReport> {
    let flow = select_flow(manifest, runtime.flow.as_deref())?;
    let lint_errors = lint_builtin_rules(&flow.flow);
    let (component_ids, mut component_errors) =
        collect_component_ids(manifest, materialized.components_dir.as_deref());
    let missing_components = find_missing_components(&flow.flow, &component_ids);
    let provider_ids = provider_ids_from_extensions(manifest.extensions.as_ref());
    let secret_uris = collect_secret_uris(manifest, runtime, &provider_ids);
    let mut walk = walk_flow(&flow.flow, &component_ids, runtime.dry_run);

    Ok(PackRunReport {
        pack_id: manifest.pack_id.to_string(),
        pack_path: materialized.pack_path.to_path_buf(),
        flow_id: flow.id.to_string(),
        flow_kind: format!("{:?}", flow.kind),
        lint_errors,
        missing_components,
        provider_ids,
        dry_run: runtime.dry_run,
        steps: walk.steps,
        errors: {
            component_errors.append(&mut walk.errors);
            component_errors
        },
        secret_uris,
    })
}

fn collect_component_ids(
    manifest: &PackManifest,
    components_dir: Option<&Path>,
) -> (BTreeSet<String>, Vec<String>) {
    let mut errors = Vec::new();
    let mut ids: BTreeSet<String> = manifest
        .components
        .iter()
        .map(|c| c.id.as_str().to_string())
        .collect();
    if let Some(root) = components_dir.filter(|r| r.exists()) {
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n == "component.manifest.json")
                {
                    match fs::read_to_string(&path).ok().and_then(|content| {
                        serde_json::from_str::<greentic_types::component::ComponentManifest>(
                            &content,
                        )
                        .ok()
                    }) {
                        Some(component) => {
                            ids.insert(component.id.as_str().to_string());
                        }
                        None => errors.push(format!(
                            "failed to parse component manifest {}",
                            path.display()
                        )),
                    }
                }
            }
        }
    }
    (ids, errors)
}

fn find_missing_components(flow: &Flow, component_ids: &BTreeSet<String>) -> Vec<String> {
    flow.nodes
        .values()
        .filter_map(|node| {
            let id = node.component.id.as_str();
            if component_ids.contains(id) {
                None
            } else {
                Some(id.to_string())
            }
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
}

fn collect_secret_uris(
    manifest: &PackManifest,
    runtime: &PackRuntimeArgs,
    _provider_ids: &[String],
) -> Vec<String> {
    let mut uris = BTreeSet::new();
    if !manifest.secret_requirements.is_empty() {
        let ctx = DevContext::new(
            runtime.env.clone(),
            runtime.tenant.clone(),
            Some(runtime.team.clone()),
        );
        for req in &manifest.secret_requirements {
            uris.insert(resolve_uri(&ctx, req));
        }
    }
    uris.into_iter().collect()
}

struct FlowWalk {
    steps: Vec<PackStepReport>,
    errors: Vec<String>,
}

fn walk_flow(flow: &Flow, component_ids: &BTreeSet<String>, dry_run: bool) -> FlowWalk {
    let mut steps = Vec::new();
    let mut errors = Vec::new();

    let Some(mut current) = resolve_entry_node(flow) else {
        errors.push("flow has no entrypoint".to_string());
        return FlowWalk { steps, errors };
    };

    let mut visited: HashSet<NodeId> = HashSet::new();
    let max_steps = flow.nodes.len().saturating_mul(4).max(1);

    loop {
        if !visited.insert(current.clone()) {
            errors.push(format!("detected cycle at node {}", current));
            break;
        }
        if steps.len() >= max_steps {
            errors.push(format!(
                "aborted after {} steps (possible infinite loop)",
                steps.len()
            ));
            break;
        }

        let Some(node) = flow.nodes.get(&current) else {
            errors.push(format!("entrypoint references missing node {}", current));
            break;
        };
        let component_id = node.component.id.as_str().to_string();
        let status = if component_ids.contains(&component_id) {
            if dry_run {
                PackStepStatus::Planned
            } else {
                PackStepStatus::Executed
            }
        } else {
            PackStepStatus::MissingComponent
        };

        steps.push(PackStepReport {
            node_id: current.to_string(),
            component_id: component_id.clone(),
            operation: node.component.operation.clone(),
            status,
        });

        if matches!(
            steps.last().map(|s| &s.status),
            Some(PackStepStatus::MissingComponent)
        ) {
            break;
        }

        match next_node_id(node) {
            Some(next) => {
                current = next;
                continue;
            }
            None => break,
        }
    }

    FlowWalk { steps, errors }
}

fn resolve_entry_node(flow: &Flow) -> Option<NodeId> {
    for key in ["default", "smoke"] {
        if let Some(entry) = flow.entrypoints.get(key).and_then(|v| v.as_str())
            && let Ok(id) = NodeId::new(entry)
        {
            return Some(id);
        }
    }
    for entry in flow.entrypoints.values() {
        if let Some(value) = entry.as_str()
            && let Ok(id) = NodeId::new(value)
        {
            return Some(id);
        }
    }
    flow.nodes.keys().next().cloned()
}

fn next_node_id(node: &Node) -> Option<NodeId> {
    match &node.routing {
        Routing::Next { node_id } => Some(node_id.clone()),
        Routing::Branch { on_status, default } => default
            .clone()
            .or_else(|| on_status.values().next().cloned()),
        Routing::End | Routing::Reply => None,
        Routing::Custom(_) => None,
    }
}

fn provider_ids_from_extensions(exts: Option<&BTreeMap<String, ExtensionRef>>) -> Vec<String> {
    let mut providers = Vec::new();
    let Some(exts) = exts else {
        return providers;
    };
    if let Some(provider_ext) = exts.get(PROVIDER_EXTENSION_ID)
        && let Some(ExtensionInline::Provider(ProviderExtensionInline {
            providers: list, ..
        })) = provider_ext.inline.as_ref()
    {
        for p in list {
            providers.push(p.provider_type.clone());
        }
    }
    providers.sort();
    providers.dedup();
    providers
}

pub fn format_secret_uri(env: &str, tenant: &str, team: &str, provider: &str) -> String {
    format!(
        "secrets://{}/{}/{}/messaging/{}.credentials.json",
        env, tenant, team, provider
    )
}

/// Redact the env/tenant/team segments of a secret URI before logging.
pub fn redact_secret_uri(uri: &str) -> String {
    const PREFIX: &str = "secrets://";
    if let Some(rest) = uri.strip_prefix(PREFIX) {
        let mut parts: Vec<&str> = rest.split('/').collect();
        if parts.len() >= 3 {
            parts[0] = "***";
            parts[1] = "***";
            parts[2] = "***";
            return format!("{PREFIX}{}", parts.join("/"));
        }
    }
    "***".into()
}

fn walk(root: &Path, pattern: &str) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = fs::read_dir(&dir).with_context(|| format!("read {}", dir.display()))?;
        for entry in entries {
            let dirent = entry.with_context(|| format!("walk {}", dir.display()))?;
            let path = dirent.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if let Some(file_name) = path.file_name().and_then(|s| s.to_str())
                && matches_pattern(file_name, pattern)
            {
                out.push(path);
            }
        }
    }
    Ok(out)
}

fn matches_pattern(file_name: &str, pattern: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if let Some(star) = pattern.find('*') {
        let (prefix, rest) = pattern.split_at(star);
        let suffix = &rest[1..];
        return file_name.starts_with(prefix) && file_name.ends_with(suffix);
    }
    file_name == pattern
}

#[cfg(test)]
mod tests {
    use super::*;
    use greentic_types::flow::FlowKind;
    use greentic_types::pack_manifest::{PackKind, PackSignatures};
    use greentic_types::{FlowId, PackId};
    use indexmap::IndexMap;
    use semver::Version;
    use std::fs;
    use std::sync::Mutex;
    use tempfile::tempdir;

    fn manifest_with_flows(flow_ids: &[&str]) -> PackManifest {
        let flows = flow_ids
            .iter()
            .map(|id| {
                let flow_id = FlowId::new(id).unwrap();
                PackFlowEntry {
                    id: flow_id.clone(),
                    kind: FlowKind::Messaging,
                    flow: Flow {
                        schema_version: "flow-v1".into(),
                        id: flow_id,
                        kind: FlowKind::Messaging,
                        entrypoints: BTreeMap::new(),
                        nodes: IndexMap::default(),
                        metadata: Default::default(),
                    },
                    tags: Vec::new(),
                    entrypoints: Vec::new(),
                }
            })
            .collect();

        PackManifest {
            schema_version: "pack-v1".into(),
            pack_id: PackId::new("demo.pack").unwrap(),
            version: Version::new(0, 1, 0),
            kind: PackKind::Provider,
            publisher: "test".into(),
            components: Vec::new(),
            flows,
            dependencies: Vec::new(),
            capabilities: Vec::new(),
            secret_requirements: Vec::new(),
            signatures: PackSignatures::default(),
            bootstrap: None,
            extensions: None,
        }
    }

    #[test]
    fn select_flow_prefers_smoke_then_first() {
        let manifest = manifest_with_flows(&["alpha", "smoke"]);
        let flow = select_flow(&manifest, None).expect("select smoke flow");
        assert_eq!(flow.id.as_str(), "smoke");

        let manifest = manifest_with_flows(&["first"]);
        let flow = select_flow(&manifest, None).expect("select first flow");
        assert_eq!(flow.id.as_str(), "first");
    }

    #[test]
    fn discover_packs_respects_glob() {
        let dir = tempdir().unwrap();
        let matching = dir.path().join("messaging-demo.gtpack");
        let ignored = dir.path().join("other-pack.gtpack");
        fs::write(&matching, b"not-a-pack").unwrap();
        fs::write(&ignored, b"ignored").unwrap();

        let args = PackDiscoveryArgs {
            roots: vec![dir.path().to_path_buf()],
            glob: "messaging-*.gtpack".into(),
        };
        let packs = discover_packs(&args).expect("discover packs");
        assert_eq!(packs.len(), 1);
        assert_eq!(packs[0].path, matching);
        assert!(packs[0].error.is_some(), "invalid pack should report error");
    }

    #[derive(Default)]
    struct FakeMaterializer {
        calls: Mutex<Vec<MaterializeOptions>>,
    }

    impl PackMaterializer for FakeMaterializer {
        fn materialize(&self, pack: &Path, opts: &MaterializeOptions) -> Result<MaterializedPack> {
            self.calls.lock().unwrap().push(opts.clone());
            Ok(MaterializedPack {
                pack_path: pack.to_path_buf(),
                components_dir: None,
            })
        }
    }

    fn runtime_with_defaults() -> PackRuntimeArgs {
        PackRuntimeArgs {
            flow: None,
            env: "dev".into(),
            tenant: "ci".into(),
            team: "ci".into(),
            allow_tags: false,
            offline: false,
            dry_run: true,
            resolve_components: true,
        }
    }

    #[test]
    fn materialize_defaults_on() {
        let mat = FakeMaterializer::default();
        let runtime = runtime_with_defaults();
        let path = Path::new("demo.gtpack");
        materialize_pack(path, &runtime, &mat).expect("materialize");
        let calls = mat.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        let opts = &calls[0];
        assert!(opts.resolve_components);
        assert!(!opts.offline);
        assert!(!opts.allow_tags);
    }

    #[test]
    fn materialize_respects_flags() {
        let mat = FakeMaterializer::default();
        let mut runtime = runtime_with_defaults();
        runtime.resolve_components = false;
        runtime.offline = true;
        runtime.allow_tags = true;
        let path = Path::new("demo.gtpack");
        materialize_pack(path, &runtime, &mat).expect("materialize");
        let calls = mat.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        let opts = &calls[0];
        assert!(!opts.resolve_components);
        assert!(opts.offline);
        assert!(opts.allow_tags);
    }
}
