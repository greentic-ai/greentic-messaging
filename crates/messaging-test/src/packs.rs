use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use greentic_flow::lint::lint_builtin_rules;
use greentic_pack::{SigningPolicy, open_pack};
use greentic_secrets::core::seed::{DevContext, resolve_uri};
use greentic_types::flow::{Node, Routing};
use greentic_types::pack_manifest::{ExtensionInline, ExtensionRef, PackFlowEntry, PackManifest};
use greentic_types::provider::{PROVIDER_EXTENSION_ID, ProviderExtensionInline};
use greentic_types::{Flow, FlowId, NodeId};

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

pub fn run_pack_from_path(path: &Path, runtime: &PackRuntimeArgs) -> Result<PackRunReport> {
    let manifest = load_manifest(path)?;
    run_pack(&manifest, runtime, path)
}

pub fn run_all_packs(
    packs: &[DiscoveredPack],
    runtime: &PackRuntimeArgs,
    fail_fast: bool,
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
        let report = run_pack(&manifest, runtime, &pack.path)?;
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
    pack_path: &Path,
) -> Result<PackRunReport> {
    let flow = select_flow(manifest, runtime.flow.as_deref())?;
    let lint_errors = lint_builtin_rules(&flow.flow);
    let component_ids: BTreeSet<String> = manifest
        .components
        .iter()
        .map(|c| c.id.as_str().to_string())
        .collect();
    let missing_components = find_missing_components(&flow.flow, &component_ids);
    let provider_ids = provider_ids_from_extensions(manifest.extensions.as_ref());
    let secret_uris = collect_secret_uris(manifest, runtime, &provider_ids);
    let walk = walk_flow(&flow.flow, &component_ids, runtime.dry_run);

    Ok(PackRunReport {
        pack_id: manifest.pack_id.to_string(),
        pack_path: pack_path.to_path_buf(),
        flow_id: flow.id.to_string(),
        flow_kind: format!("{:?}", flow.kind),
        lint_errors,
        missing_components,
        provider_ids,
        dry_run: runtime.dry_run,
        steps: walk.steps,
        errors: walk.errors,
        secret_uris,
    })
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
}
