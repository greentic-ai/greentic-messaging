use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::env;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{Context, Result, anyhow, bail};
use blake3::Hasher;
use futures::StreamExt;
use greentic_distributor_client::dist::{DistClient, DistOptions};
use greentic_flow::lint::lint_builtin_rules;
use greentic_pack::{SigningPolicy, open_pack};
use greentic_types::flow::{Node, Routing};
use greentic_types::pack::extensions::component_sources::ArtifactLocationV1;
use greentic_types::pack_manifest::{ExtensionInline, ExtensionRef, PackFlowEntry, PackManifest};
use greentic_types::provider::{PROVIDER_EXTENSION_ID, ProviderExtensionInline};
use greentic_types::{Flow, FlowId, NodeId, ProviderInstallId, ProviderInstallRecord, TenantCtx};
use secrets_core::DefaultResolver;
use secrets_core::embedded::SecretsError;
use secrets_core::resolver::ResolverConfig;
use secrets_core::seed::{DevContext, DevStore, SecretsStore, resolve_uri};
use time::OffsetDateTime;

use gsm_bus::InMemoryBusClient;
use gsm_core::{
    AdapterRegistry, EnvId, HttpRunnerClient, OutKind, OutMessage, Platform, ProviderInstallState,
    RunnerClient, apply_install_refs, infer_platform_from_adapter_name, make_tenant_ctx,
    set_current_env,
};
use gsm_egress::adapter_registry::AdapterLookup;
use gsm_egress::config::EgressConfig;
use gsm_egress::process_message_internal;

use crate::cli::{PackDiscoveryArgs, PackRuntimeArgs, RunnerTransport};

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
    pub required_config: BTreeMap<String, Vec<String>>,
    pub missing_config: BTreeMap<String, Vec<String>>,
    pub missing_secret_keys: Vec<String>,
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
            let canonical = path.canonicalize().unwrap_or(path);
            if seen.insert(canonical.clone()) {
                results.push(load_pack(&canonical));
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

pub fn run_pack_live_egress(
    path: &Path,
    runtime: &PackRuntimeArgs,
    runner_url: Option<&str>,
) -> Result<()> {
    if runtime.dry_run {
        bail!("--live requires --dry-run=false");
    }

    let provided_config = parse_runtime_config(runtime)?;
    let pack_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let manifest = load_manifest(&pack_path)?;
    let pack_root = pack_path.parent().unwrap_or_else(|| Path::new("."));
    let adapters = AdapterRegistry::load_from_paths(pack_root, std::slice::from_ref(&pack_path))
        .context("load pack adapters")?;
    let adapter = adapters
        .all()
        .into_iter()
        .find(|adapter| adapter.allows_egress())
        .ok_or_else(|| anyhow!("pack has no egress adapter"))?;
    let provider_ids = provider_ids_from_extensions(manifest.extensions.as_ref());
    let platform = infer_platform_for_live(&adapter.name, &manifest, &provider_ids);

    let chat_id = config_value(
        &provided_config,
        &["chat-id", "chat_id", "channel-id", "channel_id"],
    )
    .ok_or_else(|| anyhow!("missing required config: chat-id"))?;
    let text = config_value(&provided_config, &["text", "message"])
        .unwrap_or_else(|| "live pack test".into());
    let thread_id = config_value(&provided_config, &["thread-id", "thread_id"]);

    set_current_env(EnvId(runtime.env.clone()));
    let tenant_ctx = make_tenant_ctx(runtime.tenant.clone(), Some(runtime.team.clone()), None);
    let out = OutMessage {
        ctx: tenant_ctx.clone(),
        tenant: runtime.tenant.clone(),
        platform,
        chat_id,
        thread_id,
        kind: OutKind::Text,
        text: Some(text),
        message_card: None,
        adaptive_card: None,
        meta: Default::default(),
    };

    let provider_id = provider_ids
        .first()
        .ok_or_else(|| anyhow!("pack has no provider id"))?
        .clone();

    let required_config = required_provider_config(&pack_path, manifest.extensions.as_ref())?;
    let secret_uris = collect_secret_uris(&manifest, runtime, &provider_ids);
    let missing_config = missing_required_config(&required_config, &provided_config);
    let resolved_secrets = resolve_secret_values(&secret_uris, &provided_config, runtime, true)?;
    let missing_secret_keys = missing_secret_keys(&secret_uris, &resolved_secrets);
    if !missing_config.is_empty() || !missing_secret_keys.is_empty() {
        bail!("missing config/secret values for live run");
    }

    let install_state = build_install_state_for_live(LiveInstallInputs {
        tenant_ctx: &tenant_ctx,
        provider_id: &provider_id,
        manifest: &manifest,
        channel_id: &out.chat_id,
        config_values: &provided_config,
        secret_values: &resolved_secrets,
        required_config: &required_config,
        secret_uris: &secret_uris,
    })?;

    let config = EgressConfig {
        env: tenant_ctx.env.clone(),
        nats_url: nats_url_from_env(runtime),
        subject_filter: format!("{}.{}.>", gsm_core::EGRESS_SUBJECT_PREFIX, tenant_ctx.env.0),
        adapter: None,
        packs_root: ".".into(),
        egress_prefix: gsm_core::EGRESS_SUBJECT_PREFIX.to_string(),
        runner_http_url: runner_url
            .map(|value| value.to_string())
            .or_else(|| Some(runner_url_from_env())),
        runner_http_api_key: None,
        install_store_path: None,
    };

    match runtime.runner_transport {
        RunnerTransport::Http => {
            let runner: Box<dyn RunnerClient> = Box::new(HttpRunnerClient::new(
                runner_url
                    .map(|value| value.to_string())
                    .unwrap_or_else(runner_url_from_env),
                None,
            )?);
            let lookup = AdapterLookup::new(&adapters);
            let resolved = lookup.egress(&adapter.name)?;
            let bus = InMemoryBusClient::default();
            let tokio_runtime = tokio::runtime::Runtime::new().context("build runtime")?;

            tokio_runtime
                .block_on(async {
                    tokio::time::timeout(
                        std::time::Duration::from_secs(10),
                        process_message_internal(
                            &out,
                            &resolved,
                            &bus,
                            runner.as_ref(),
                            &config,
                            &install_state,
                        ),
                    )
                    .await
                })
                .context("egress timeout")?
                .context("egress failed")?;
        }
        RunnerTransport::Nats => {
            let mut routed = out.clone();
            apply_install_refs(&mut routed.meta, &install_state.record);
            let subject = gsm_core::egress_subject_with_prefix(
                gsm_core::EGRESS_SUBJECT_PREFIX,
                tenant_ctx.env.as_str(),
                tenant_ctx.tenant.as_str(),
                tenant_ctx
                    .team
                    .as_ref()
                    .map(|t| t.as_str())
                    .unwrap_or("default"),
                routed.platform.as_str(),
            );
            let nats_url = nats_url_from_env(runtime);
            println!(
                "  live: publishing egress message to {} via {}",
                subject, nats_url
            );
            let payload = serde_json::to_vec(&routed)?;
            let tokio_runtime = tokio::runtime::Runtime::new().context("build runtime")?;
            tokio_runtime
                .block_on(async {
                    let client = async_nats::connect(nats_url).await?;
                    client.publish(subject, payload.into()).await?;
                    client.flush().await?;
                    Ok::<(), anyhow::Error>(())
                })
                .context("nats publish failed")?;
        }
    }

    Ok(())
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

pub fn listen_egress(runtime: &PackRuntimeArgs) -> Result<()> {
    let subject = egress_wildcard_subject(runtime);
    let nats_url = nats_url_from_env(runtime);
    println!("Listening for egress messages on {subject} via {nats_url}");

    let tokio_runtime = tokio::runtime::Runtime::new().context("build runtime")?;
    tokio_runtime.block_on(async {
        let client = async_nats::connect(nats_url).await?;
        let mut subscription = client.subscribe(subject.clone()).await?;
        while let Some(message) = subscription.next().await {
            let timestamp = OffsetDateTime::now_utc().unix_timestamp();
            let payload = match std::str::from_utf8(&message.payload) {
                Ok(text) => text.to_string(),
                Err(_) => format!("<{} bytes>", message.payload.len()),
            };
            println!("[{timestamp}] {} {}", message.subject, payload);
        }
        Ok::<(), anyhow::Error>(())
    })?;
    Ok(())
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
    let provided_config = parse_runtime_config(runtime)?;
    let required_config = required_provider_config(
        materialized.pack_path.as_path(),
        manifest.extensions.as_ref(),
    )?;
    let missing_config = missing_required_config(&required_config, &provided_config);
    let resolved_secrets =
        resolve_secret_values(&secret_uris, &provided_config, runtime, runtime.live)?;
    let missing_secret_keys = missing_secret_keys(&secret_uris, &resolved_secrets)
        .into_iter()
        .collect();

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
        required_config,
        missing_config,
        missing_secret_keys,
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

fn parse_runtime_config(runtime: &PackRuntimeArgs) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for entry in &runtime.config {
        let trimmed = entry.trim();
        if trimmed.is_empty() {
            continue;
        }
        let (key, value) = trimmed
            .split_once('=')
            .ok_or_else(|| anyhow!("invalid --config entry '{trimmed}' (expected key=value)"))?;
        let key = key.trim();
        if key.is_empty() {
            bail!("invalid --config entry '{trimmed}' (empty key)");
        }
        out.insert(key.to_string(), value.trim().to_string());
    }
    Ok(out)
}

fn config_value(map: &BTreeMap<String, String>, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(value) = map.get(*key) {
            return Some(value.clone());
        }
    }
    None
}

fn required_provider_config(
    pack_path: &Path,
    exts: Option<&BTreeMap<String, ExtensionRef>>,
) -> Result<BTreeMap<String, Vec<String>>> {
    let mut out = BTreeMap::new();
    let Some(exts) = exts else {
        return Ok(out);
    };
    let Some(provider_ext) = exts.get(PROVIDER_EXTENSION_ID) else {
        return Ok(out);
    };
    let Some(ExtensionInline::Provider(ProviderExtensionInline { providers, .. })) =
        provider_ext.inline.as_ref()
    else {
        return Ok(out);
    };

    for provider in providers {
        let schema_ref = provider.config_schema_ref.trim();
        if schema_ref.is_empty() {
            out.insert(provider.provider_type.clone(), Vec::new());
            continue;
        }
        let required = match load_pack_json_schema(pack_path, schema_ref) {
            Ok(schema) => extract_required_keys(&schema),
            Err(_) => Vec::new(),
        };
        out.insert(provider.provider_type.clone(), required);
    }
    Ok(out)
}

fn missing_required_config(
    required: &BTreeMap<String, Vec<String>>,
    provided: &BTreeMap<String, String>,
) -> BTreeMap<String, Vec<String>> {
    let mut missing = BTreeMap::new();
    for (provider, keys) in required {
        let mut needed = Vec::new();
        for key in keys {
            if !provided.contains_key(key) {
                needed.push(key.clone());
            }
        }
        if !needed.is_empty() {
            missing.insert(provider.clone(), needed);
        }
    }
    missing
}

fn resolve_secret_values(
    secret_uris: &[String],
    provided: &BTreeMap<String, String>,
    runtime: &PackRuntimeArgs,
    use_store: bool,
) -> Result<BTreeMap<String, String>> {
    let mut resolved = provided.clone();
    if !use_store || secret_uris.is_empty() {
        return Ok(resolved);
    }

    let runtime_rt = tokio::runtime::Runtime::new().context("build secrets runtime")?;

    set_dev_store_envs();
    if let Ok(store) = DevStore::open_default() {
        for uri in secret_uris {
            let Some(key) = secret_key_from_uri(uri) else {
                continue;
            };
            if resolved.contains_key(&key) {
                continue;
            }
            let fetched = runtime_rt.block_on(async { store.get(uri).await });
            match fetched {
                Ok(bytes) => match String::from_utf8(bytes) {
                    Ok(value) => {
                        resolved.insert(key, value);
                    }
                    Err(err) => {
                        eprintln!("warning: secret {} is not valid UTF-8: {}", uri, err);
                    }
                },
                Err(err) => {
                    eprintln!("warning: failed to read secret {uri}: {err}");
                }
            }
        }
        return Ok(resolved);
    }

    let resolver = runtime_rt.block_on(async {
        let config = ResolverConfig::from_env()
            .tenant(runtime.tenant.clone())
            .team(runtime.team.clone());
        DefaultResolver::from_config(config).await
    });
    let resolver = match resolver {
        Ok(resolver) => resolver,
        Err(err) => {
            eprintln!("warning: secrets resolver unavailable: {err}");
            return Ok(resolved);
        }
    };

    for uri in secret_uris {
        let Some(key) = secret_key_from_uri(uri) else {
            continue;
        };
        if resolved.contains_key(&key) {
            continue;
        }
        let fetched = runtime_rt.block_on(async { resolver.get_text(uri).await });
        match fetched {
            Ok(value) => {
                resolved.insert(key, value);
            }
            Err(SecretsError::Core(core)) if format!("{core}").contains("NotFound") => {}
            Err(err) => {
                eprintln!("warning: failed to read secret {uri}: {err}");
            }
        }
    }

    Ok(resolved)
}

fn set_dev_store_envs() {
    if std::env::var("GREENTIC_DEV_SECRETS_PATH").is_ok() {
        return;
    }
    let mut candidates = Vec::new();
    if let Ok(home) = std::env::var("GREENTIC_HOME") {
        let base = PathBuf::from(home);
        candidates.push(base.join("dev/.dev.secrets.env"));
        candidates.push(base.join(".dev.secrets.env"));
    }
    candidates.push(PathBuf::from(".greentic/dev/.dev.secrets.env"));
    candidates.push(PathBuf::from(".dev.secrets.env"));

    for path in candidates {
        if path.exists() {
            unsafe {
                std::env::set_var("GREENTIC_DEV_SECRETS_PATH", &path);
            }
            break;
        }
    }
}

fn load_pack_json_schema(pack_path: &Path, schema_ref: &str) -> Result<serde_json::Value> {
    let bytes = read_pack_file_ref(pack_path, schema_ref)?;
    let schema =
        serde_json::from_slice(&bytes).with_context(|| format!("parse schema {}", schema_ref))?;
    Ok(schema)
}

fn read_pack_file_ref(pack_path: &Path, file_ref: &str) -> Result<Vec<u8>> {
    let ext = pack_path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase());
    if ext.as_deref() == Some("gtpack") {
        let file = File::open(pack_path)?;
        let mut archive = zip::ZipArchive::new(file)?;
        let mut buf = Vec::new();
        archive
            .by_name(file_ref)
            .with_context(|| format!("missing {}", file_ref))?
            .read_to_end(&mut buf)
            .with_context(|| format!("read {}", file_ref))?;
        Ok(buf)
    } else {
        let base = pack_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(file_ref);
        let mut buf = Vec::new();
        File::open(&base)
            .with_context(|| format!("open {}", base.display()))?
            .read_to_end(&mut buf)
            .with_context(|| format!("read {}", base.display()))?;
        Ok(buf)
    }
}

fn extract_required_keys(schema: &serde_json::Value) -> Vec<String> {
    schema
        .get("required")
        .and_then(|value| value.as_array())
        .map(|list| {
            list.iter()
                .filter_map(|item| item.as_str().map(|s| s.to_string()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn missing_secret_keys(
    secret_uris: &[String],
    provided: &BTreeMap<String, String>,
) -> BTreeSet<String> {
    let mut missing = BTreeSet::new();
    for uri in secret_uris {
        if let Some(key) = secret_key_from_uri(uri)
            && !provided.contains_key(&key)
        {
            missing.insert(key);
        }
    }
    missing
}

fn secret_key_from_uri(uri: &str) -> Option<String> {
    let key = uri.split('/').next_back()?.trim();
    if key.is_empty() {
        None
    } else {
        Some(key.to_string())
    }
}

struct LiveInstallInputs<'a> {
    tenant_ctx: &'a TenantCtx,
    provider_id: &'a str,
    manifest: &'a PackManifest,
    channel_id: &'a str,
    config_values: &'a BTreeMap<String, String>,
    secret_values: &'a BTreeMap<String, String>,
    required_config: &'a BTreeMap<String, Vec<String>>,
    secret_uris: &'a [String],
}

fn build_install_state_for_live(inputs: LiveInstallInputs<'_>) -> Result<ProviderInstallState> {
    let install_id = ProviderInstallId::new("live-test").expect("static install id");
    let now = OffsetDateTime::now_utc();
    let mut config_refs = BTreeMap::new();
    if let Some(required) = inputs.required_config.get(inputs.provider_id) {
        for key in required {
            config_refs.insert(key.to_string(), format!("config:{key}"));
        }
    }
    let mut secret_refs = BTreeMap::new();
    for uri in inputs.secret_uris {
        if let Some(key) = secret_key_from_uri(uri) {
            secret_refs.insert(key, uri.to_string());
        }
    }

    let record = ProviderInstallRecord {
        tenant: inputs.tenant_ctx.clone(),
        provider_id: inputs.provider_id.to_string(),
        install_id: install_id.clone(),
        pack_id: inputs.manifest.pack_id.clone(),
        pack_version: inputs.manifest.version.clone(),
        created_at: now,
        updated_at: now,
        config_refs,
        secret_refs,
        webhook_state: serde_json::json!({}),
        subscriptions_state: serde_json::json!({}),
        metadata: serde_json::json!({
            "routing": {
                "platform": infer_platform_from_provider(inputs.provider_id)
                    .unwrap_or("unknown".into()),
                "channel_id": inputs.channel_id,
            }
        }),
    };

    let mut config = BTreeMap::new();
    for (key, value) in inputs.config_values {
        config.insert(
            key.to_string(),
            serde_json::Value::String(value.to_string()),
        );
    }
    let mut secrets = BTreeMap::new();
    for key in record.secret_refs.keys() {
        if let Some(value) = inputs.secret_values.get(key) {
            secrets.insert(key.to_string(), value.to_string());
        }
    }

    Ok(ProviderInstallState::new(record)
        .with_config(config)
        .with_secrets(secrets))
}

fn infer_platform_from_provider(provider_id: &str) -> Option<String> {
    let provider = provider_id.to_ascii_lowercase();
    if provider.contains("slack") {
        Some("slack".into())
    } else if provider.contains("teams") {
        Some("teams".into())
    } else if provider.contains("webex") {
        Some("webex".into())
    } else if provider.contains("webchat") {
        Some("webchat".into())
    } else if provider.contains("whatsapp") {
        Some("whatsapp".into())
    } else if provider.contains("telegram") {
        Some("telegram".into())
    } else {
        None
    }
}

fn infer_platform_for_live(
    adapter_name: &str,
    manifest: &PackManifest,
    provider_ids: &[String],
) -> Platform {
    infer_platform_from_adapter_name(adapter_name)
        .or_else(|| {
            infer_platform_from_provider(manifest.pack_id.as_str())
                .and_then(|value| Platform::from_str(&value).ok())
        })
        .or_else(|| {
            provider_ids
                .first()
                .and_then(|id| infer_platform_from_provider(id))
                .and_then(|value| Platform::from_str(&value).ok())
        })
        .unwrap_or(Platform::Slack)
}

fn runner_url_from_env() -> String {
    env::var("RUNNER_URL").unwrap_or_else(|_| "http://localhost:8081/invoke".into())
}

fn nats_url_from_env(runtime: &PackRuntimeArgs) -> String {
    runtime
        .nats_url
        .clone()
        .unwrap_or_else(|| env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into()))
}

fn egress_wildcard_subject(runtime: &PackRuntimeArgs) -> String {
    format!(
        "{}.{}.{}.{}.>",
        gsm_core::EGRESS_SUBJECT_PREFIX,
        subject_token(&runtime.env),
        subject_token(&runtime.tenant),
        subject_token(&runtime.team)
    )
}

fn subject_token(value: &str) -> String {
    let mut sanitized = value
        .trim()
        .replace([' ', '\t', '\n', '\r', '*', '>', '/'], "-");
    if sanitized.is_empty() {
        sanitized = "unknown".into();
    }
    sanitized
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
            live: false,
            config: Vec::new(),
            runner_transport: RunnerTransport::Http,
            nats_url: None,
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
