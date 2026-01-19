use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use handlebars::Handlebars;
use regex::Regex;
use serde::Deserialize;
use serde_json::{Value, json};

use greentic_messaging_validate::messaging_validators;
use greentic_pack::messaging::MessagingAdapter;
use greentic_pack::reader::{SigningPolicy, open_pack};
use greentic_types::pack_manifest::{ExtensionInline, PackManifest};
use greentic_types::validate::{Diagnostic, Severity};
use gsm_core::{
    AdapterDescriptor, AdapterRegistry, MessageEnvelope, MessagingAdapterKind, OutKind, OutMessage,
    Platform, RunnerClient, make_tenant_ctx, set_current_env,
};

use crate::cli::{PackDiscoveryArgs, PackRuntimeArgs};
use crate::packs::{DiscoveredPack, discover_packs};

#[derive(Debug)]
pub struct ConformanceReport {
    pub pack_id: String,
    pub pack_path: PathBuf,
    pub steps: Vec<ConformanceStep>,
}

impl ConformanceReport {
    pub fn is_success(&self) -> bool {
        self.steps
            .iter()
            .all(|step| step.status == ConformanceStatus::Ok)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConformanceStatus {
    Ok,
    Skipped,
    Failed,
}

#[derive(Debug)]
pub struct ConformanceStep {
    pub name: &'static str,
    pub status: ConformanceStatus,
    pub details: Vec<String>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ToolCall {
    pub tool: String,
    pub action: String,
    pub input: Value,
}

#[derive(Debug)]
struct FlowRunOutcome {
    out_messages: Vec<OutMessage>,
    tool_calls: Vec<ToolCall>,
}

pub struct ConformanceOptions {
    pub discovery: PackDiscoveryArgs,
    pub runtime: PackRuntimeArgs,
    pub pack_paths: Vec<PathBuf>,
    pub public_base_url: String,
    pub ingress_fixture: PathBuf,
    pub setup_only: bool,
}

pub fn run_conformance(options: ConformanceOptions) -> Result<Vec<ConformanceReport>> {
    set_current_env(gsm_core::EnvId(options.runtime.env.clone()));
    let packs = if options.pack_paths.is_empty() {
        discover_packs(&options.discovery)?
    } else {
        load_override_packs(&options.pack_paths)
    };
    if packs.is_empty() {
        return Ok(Vec::new());
    }

    let pack_paths = packs
        .iter()
        .map(|pack| pack.path.clone())
        .collect::<Vec<_>>();
    let registry_root = if let Some(root) = options.discovery.roots.first() {
        root.clone()
    } else {
        pack_paths
            .first()
            .and_then(|path| path.parent().map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("."))
    };
    let registry = AdapterRegistry::load_from_paths(&registry_root, &pack_paths)
        .context("load pack adapters for conformance")?;

    let mut reports = Vec::new();
    for pack in packs {
        if let Some(err) = pack.error {
            reports.push(ConformanceReport {
                pack_id: pack.path.display().to_string(),
                pack_path: pack.path.clone(),
                steps: vec![ConformanceStep {
                    name: "requirements",
                    status: ConformanceStatus::Failed,
                    details: vec![err],
                }],
            });
            continue;
        }
        let report = run_conformance_for_pack(&pack, &registry, &options)?;
        reports.push(report);
    }
    Ok(reports)
}

fn run_conformance_for_pack(
    pack: &DiscoveredPack,
    registry: &AdapterRegistry,
    options: &ConformanceOptions,
) -> Result<ConformanceReport> {
    let mut steps = Vec::new();

    let manifest = pack.manifest.clone();
    let pack_id = pack_id_from_manifest_or_path(manifest.as_ref(), &pack.path);
    steps.push(run_requirements_step(manifest.as_ref()));

    let flow_root = pack
        .path
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let flows = FlowRegistry::load_from_paths(&flow_root, std::slice::from_ref(&pack.path))?;

    let mut fixture_env = load_fixture_envelope(&options.ingress_fixture)?;
    fixture_env.tenant = options.runtime.tenant.clone();
    fixture_env.context.insert(
        "team".to_string(),
        Value::String(options.runtime.team.clone()),
    );

    if let Some(setup_flow_id) = setup_flow_id(manifest.as_ref()) {
        steps.push(run_setup_step(
            &flows,
            setup_flow_id.as_str(),
            &options.public_base_url,
            &fixture_env,
        ));
    } else {
        steps.push(ConformanceStep {
            name: "setup",
            status: ConformanceStatus::Skipped,
            details: vec!["no setup flow declared".to_string()],
        });
    }

    let pack_adapters = adapters_for_pack(registry, &pack_id);
    if !options.setup_only {
        steps.push(run_ingress_step(&flows, &pack_adapters, &fixture_env));
        steps.push(run_egress_step(&pack_adapters, &fixture_env));
        steps.push(run_subscriptions_step(
            &flows,
            manifest.as_ref(),
            &pack_id,
            &fixture_env,
        ));
    }

    Ok(ConformanceReport {
        pack_id,
        pack_path: pack.path.clone(),
        steps,
    })
}

fn run_requirements_step(manifest: Option<&PackManifest>) -> ConformanceStep {
    let Some(manifest) = manifest else {
        return ConformanceStep {
            name: "requirements",
            status: ConformanceStatus::Skipped,
            details: vec!["pack manifest unavailable; skipping validators".to_string()],
        };
    };
    let mut diagnostics = Vec::new();
    for validator in messaging_validators() {
        diagnostics.extend(validator.validate(manifest));
    }
    let mut details = Vec::new();
    let mut failed = false;
    for Diagnostic {
        code,
        message,
        severity,
        ..
    } in diagnostics
    {
        details.push(format!("{}: {}", code, message));
        if matches!(severity, Severity::Error) {
            failed = true;
        }
    }
    ConformanceStep {
        name: "requirements",
        status: if failed {
            ConformanceStatus::Failed
        } else {
            ConformanceStatus::Ok
        },
        details,
    }
}

fn run_setup_step(
    flows: &FlowRegistry,
    flow_id: &str,
    public_base_url: &str,
    fixture: &MessageEnvelope,
) -> ConformanceStep {
    let mut details = Vec::new();
    let Some(flow) = flows.get_flow(flow_id) else {
        return ConformanceStep {
            name: "setup",
            status: ConformanceStatus::Failed,
            details: vec![format!("setup flow {} not found in pack", flow_id)],
        };
    };
    let env = with_public_base_url(fixture.clone(), public_base_url);
    match run_flow(&flow.flow, &env) {
        Ok(outcome) => {
            let (has_secrets, has_config) = detect_setup_writes(&outcome.tool_calls);
            for message in &outcome.out_messages {
                match message.kind {
                    OutKind::Text => {
                        if let Some(text) = message.text.as_ref() {
                            details.push(format!("output text: {}", text.trim()));
                        }
                    }
                    OutKind::Card => {
                        let title = message
                            .message_card
                            .as_ref()
                            .and_then(|card| card.title.clone())
                            .unwrap_or_else(|| "(untitled card)".into());
                        details.push(format!("output card: {}", title));
                    }
                }
            }
            if !has_secrets {
                details.push("no secrets write detected in tool calls".to_string());
            }
            if !has_config {
                details.push("no config write detected in tool calls".to_string());
            }
            if has_secrets && has_config {
                ConformanceStep {
                    name: "setup",
                    status: ConformanceStatus::Ok,
                    details,
                }
            } else {
                if outcome.tool_calls.is_empty() {
                    details.push("no tool calls recorded during setup flow".to_string());
                } else {
                    details.extend(
                        outcome
                            .tool_calls
                            .iter()
                            .map(|call| format!("tool {}.{} invoked", call.tool, call.action)),
                    );
                }
                ConformanceStep {
                    name: "setup",
                    status: ConformanceStatus::Failed,
                    details,
                }
            }
        }
        Err(err) => ConformanceStep {
            name: "setup",
            status: ConformanceStatus::Failed,
            details: vec![err.to_string()],
        },
    }
}

fn run_ingress_step(
    flows: &FlowRegistry,
    adapters: &[AdapterDescriptor],
    fixture: &MessageEnvelope,
) -> ConformanceStep {
    let mut details = Vec::new();
    let mut failed = false;
    let ingress_adapters = adapters
        .iter()
        .filter(|adapter| {
            matches!(
                adapter.kind,
                MessagingAdapterKind::Ingress | MessagingAdapterKind::IngressEgress
            )
        })
        .cloned()
        .collect::<Vec<_>>();

    if ingress_adapters.is_empty() {
        return ConformanceStep {
            name: "ingress",
            status: ConformanceStatus::Skipped,
            details: vec!["no ingress adapters found".to_string()],
        };
    }

    for adapter in ingress_adapters {
        match run_flow_for_adapter(flows, &adapter, fixture) {
            Ok(outcome) => {
                if outcome.out_messages.is_empty() {
                    failed = true;
                    details.push(format!("{} produced no outbound messages", adapter.name));
                } else {
                    details.push(format!(
                        "{} produced {} outbound message(s)",
                        adapter.name,
                        outcome.out_messages.len()
                    ));
                }
            }
            Err(err) => {
                failed = true;
                details.push(format!("{} failed: {}", adapter.name, err));
            }
        }
    }

    ConformanceStep {
        name: "ingress",
        status: if failed {
            ConformanceStatus::Failed
        } else {
            ConformanceStatus::Ok
        },
        details,
    }
}

fn run_egress_step(adapters: &[AdapterDescriptor], fixture: &MessageEnvelope) -> ConformanceStep {
    let mut details = Vec::new();
    let mut failed = false;
    let egress_adapters = adapters
        .iter()
        .filter(|adapter| {
            matches!(
                adapter.kind,
                MessagingAdapterKind::Egress | MessagingAdapterKind::IngressEgress
            )
        })
        .cloned()
        .collect::<Vec<_>>();

    if egress_adapters.is_empty() {
        return ConformanceStep {
            name: "egress",
            status: ConformanceStatus::Skipped,
            details: vec!["no egress adapters found".to_string()],
        };
    }

    let runner = gsm_core::LoggingRunnerClient;
    for adapter in egress_adapters {
        let out = build_stub_out_message(fixture, &adapter);
        if let Err(err) = futures::executor::block_on(runner.invoke_adapter(&out, &adapter)) {
            failed = true;
            details.push(format!("{} failed: {}", adapter.name, err));
        } else {
            details.push(format!("{} invoked (dry-run)", adapter.name));
        }
    }

    ConformanceStep {
        name: "egress",
        status: if failed {
            ConformanceStatus::Failed
        } else {
            ConformanceStatus::Ok
        },
        details,
    }
}

fn run_subscriptions_step(
    flows: &FlowRegistry,
    manifest: Option<&PackManifest>,
    pack_id: &str,
    fixture: &MessageEnvelope,
) -> ConformanceStep {
    let mut details = Vec::new();
    let flow_id = subscriptions_flow_id(manifest);
    let require = pack_id.contains("teams");

    let Some(flow_id) = flow_id else {
        return ConformanceStep {
            name: "subscriptions",
            status: if require {
                ConformanceStatus::Failed
            } else {
                ConformanceStatus::Skipped
            },
            details: vec![format!(
                "no subscriptions flow found{}",
                if require {
                    " (teams pack requires it)"
                } else {
                    ""
                }
            )],
        };
    };

    let Some(flow) = flows.get_flow(&flow_id) else {
        return ConformanceStep {
            name: "subscriptions",
            status: ConformanceStatus::Failed,
            details: vec![format!("subscriptions flow {} not found in pack", flow_id)],
        };
    };

    match run_flow(&flow.flow, fixture) {
        Ok(outcome) => {
            if outcome.out_messages.is_empty() {
                details.push("subscriptions flow produced no outbound messages".to_string());
            }
            ConformanceStep {
                name: "subscriptions",
                status: ConformanceStatus::Ok,
                details,
            }
        }
        Err(err) => ConformanceStep {
            name: "subscriptions",
            status: ConformanceStatus::Failed,
            details: vec![err.to_string()],
        },
    }
}

fn setup_flow_id(manifest: Option<&PackManifest>) -> Option<String> {
    let manifest = manifest?;
    let flow_ids: HashSet<String> = manifest.flows.iter().map(|f| f.id.to_string()).collect();
    if let Some(ext) = manifest.extensions.as_ref()
        && let Some(entry) = ext.get("messaging.provider_flow_hints")
        && let Some(inline) = entry.inline.as_ref()
        && let ExtensionInline::Other(value) = inline
        && let Ok(payload) = serde_json::from_value::<ProviderFlowHintsPayload>(value.clone())
    {
        for hint_set in payload.providers.values() {
            if let Some(flow_id) = hint_set
                .setup_default
                .as_ref()
                .or(hint_set.setup_custom.as_ref())
                && flow_ids.contains(flow_id)
            {
                return Some(flow_id.clone());
            }
        }
    }
    manifest
        .flows
        .iter()
        .find(|flow| flow_has_entrypoint(flow, "setup") || flow.id.as_str().starts_with("setup"))
        .map(|flow| flow.id.to_string())
}

fn subscriptions_flow_id(manifest: Option<&PackManifest>) -> Option<String> {
    let manifest = manifest?;
    manifest
        .flows
        .iter()
        .find(|flow| {
            flow.id.as_str().contains("subscriptions") || flow_has_entrypoint(flow, "subscriptions")
        })
        .map(|flow| flow.id.to_string())
}

fn adapters_for_pack(registry: &AdapterRegistry, pack_id: &str) -> Vec<AdapterDescriptor> {
    registry
        .all()
        .into_iter()
        .filter(|adapter| adapter.pack_id == pack_id)
        .collect()
}

fn pack_id_from_manifest_or_path(manifest: Option<&PackManifest>, path: &Path) -> String {
    if let Some(manifest) = manifest {
        return manifest.pack_id.to_string();
    }
    if path.extension().and_then(|s| s.to_str()) != Some("gtpack")
        && let Ok(raw) = fs::read_to_string(path)
        && let Ok(spec) = serde_yaml::from_str::<PackSpec>(&raw)
    {
        return spec.id;
    }
    path.display().to_string()
}

fn flow_has_entrypoint(flow: &greentic_types::pack_manifest::PackFlowEntry, entry: &str) -> bool {
    flow.entrypoints.iter().any(|id| id.as_str() == entry)
        || flow
            .flow
            .entrypoints
            .values()
            .any(|value| value.as_str() == Some(entry))
}

fn detect_setup_writes(calls: &[ToolCall]) -> (bool, bool) {
    let mut has_secrets = false;
    let mut has_config = false;
    for call in calls {
        let tool = call.tool.to_ascii_lowercase();
        let action = call.action.to_ascii_lowercase();
        if tool.contains("secrets") && action.contains("write") {
            has_secrets = true;
        }
        if tool.contains("config") && action.contains("write") {
            has_config = true;
        }
        if tool.contains("secrets") && action.contains("set") {
            has_secrets = true;
        }
        if tool.contains("config") && action.contains("set") {
            has_config = true;
        }
    }
    (has_secrets, has_config)
}

fn with_public_base_url(mut env: MessageEnvelope, url: &str) -> MessageEnvelope {
    env.context.insert(
        "public_base_url".to_string(),
        Value::String(url.to_string()),
    );
    env.context.insert(
        "PUBLIC_BASE_URL".to_string(),
        Value::String(url.to_string()),
    );
    env
}

fn load_fixture_envelope(path: &Path) -> Result<MessageEnvelope> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("read ingress fixture {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("parse ingress fixture {}", path.display()))
}

fn run_flow_for_adapter(
    flows: &FlowRegistry,
    adapter: &AdapterDescriptor,
    fixture: &MessageEnvelope,
) -> Result<FlowRunOutcome> {
    let flow = flows
        .get_flow_by_route(&adapter.name)
        .ok_or_else(|| anyhow!("flow for adapter {} not found", adapter.name))?;
    run_flow(&flow.flow, fixture)
}

fn build_stub_out_message(fixture: &MessageEnvelope, adapter: &AdapterDescriptor) -> OutMessage {
    let ctx = make_tenant_ctx(fixture.tenant.clone(), None, Some(fixture.user_id.clone()));
    OutMessage {
        ctx,
        tenant: fixture.tenant.clone(),
        platform: infer_platform_from_adapter_name(&adapter.name).unwrap_or(Platform::Slack),
        chat_id: fixture.chat_id.clone(),
        thread_id: fixture.thread_id.clone(),
        kind: OutKind::Text,
        text: Some("conformance dry-run".to_string()),
        message_card: None,
        adaptive_card: None,
        meta: {
            let mut map = BTreeMap::new();
            map.insert("source".into(), Value::String("conformance".into()));
            map
        },
    }
}

#[derive(Debug, Deserialize)]
struct ProviderFlowHintsPayload {
    #[serde(flatten)]
    providers: BTreeMap<String, ProviderFlowHintSet>,
}

#[derive(Debug, Deserialize)]
struct ProviderFlowHintSet {
    setup_default: Option<String>,
    setup_custom: Option<String>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct FlowDefinition {
    pub pack_id: String,
    pub flow_id: String,
    pub platform: Option<String>,
    pub route: Option<String>,
    pub flow: Flow,
}

#[derive(Debug, Default)]
pub struct FlowRegistry {
    flows: Vec<FlowDefinition>,
    by_route: HashMap<String, usize>,
    by_id: HashMap<String, usize>,
}

impl FlowRegistry {
    pub fn load_from_paths(root: &Path, paths: &[PathBuf]) -> Result<Self> {
        let root = root
            .canonicalize()
            .with_context(|| format!("failed to canonicalize packs root {}", root.display()))?;
        let mut flows = Vec::new();
        for path in paths {
            let pack_path = resolve_pack_path(&root, path)?;
            let ext = pack_path
                .extension()
                .and_then(|s| s.to_str())
                .map(|s| s.to_ascii_lowercase());
            match ext.as_deref() {
                Some("gtpack") => flows.extend(flows_from_gtpack(&pack_path)?),
                _ => flows.extend(flows_from_pack_yaml(&root, &pack_path)?),
            }
        }

        let mut registry = FlowRegistry::default();
        for (idx, flow) in flows.into_iter().enumerate() {
            if let Some(route) = flow.route.as_ref() {
                registry.by_route.insert(route.clone(), idx);
            }
            registry.by_id.insert(flow.flow_id.clone(), idx);
            registry.flows.push(flow);
        }
        Ok(registry)
    }

    pub fn get_flow(&self, flow_id: &str) -> Option<&FlowDefinition> {
        self.by_id.get(flow_id).and_then(|idx| self.flows.get(*idx))
    }

    pub fn get_flow_by_route(&self, route: &str) -> Option<&FlowDefinition> {
        self.by_route
            .get(route)
            .and_then(|idx| self.flows.get(*idx))
    }
}

fn resolve_pack_path(root: &Path, path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        let canonical = path
            .canonicalize()
            .with_context(|| format!("failed to canonicalize {}", path.display()))?;
        Ok(canonical)
    } else {
        gsm_core::path_safety::normalize_under_root(root, path)
    }
}

fn flows_from_pack_yaml(root: &Path, path: &Path) -> Result<Vec<FlowDefinition>> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read pack file {}", path.display()))?;
    let spec: PackSpec = serde_yaml::from_str(&raw)
        .with_context(|| format!("{} is not a valid pack spec", path.display()))?;

    let mut flows = Vec::new();
    let pack_dir = path
        .parent()
        .ok_or_else(|| anyhow!("pack path missing parent: {}", path.display()))?;
    let mut adapters = spec.messaging.and_then(|m| m.adapters).unwrap_or_default();
    adapters.sort_by(|a, b| a.name.cmp(&b.name));

    let mut flow_cache: HashMap<PathBuf, Flow> = HashMap::new();

    for adapter in adapters {
        let flow_path = adapter
            .custom_flow
            .as_ref()
            .or(adapter.default_flow.as_ref());
        let Some(flow_path) = flow_path else {
            continue;
        };
        let resolved = resolve_flow_path(root, pack_dir, Path::new(flow_path))?;
        let flow = if let Some(existing) = flow_cache.get(&resolved) {
            existing.clone()
        } else {
            let loaded = Flow::load_from_file(resolved.to_str().unwrap())?;
            flow_cache.insert(resolved.clone(), loaded.clone());
            loaded
        };
        flows.push(flow_definition_from_adapter(
            spec.id.clone(),
            &adapter,
            flow.id.clone(),
            flow,
        ));
    }

    Ok(flows)
}

fn flows_from_gtpack(path: &Path) -> Result<Vec<FlowDefinition>> {
    let pack = open_pack(path, SigningPolicy::DevOk)
        .map_err(|err| anyhow!(err.message))
        .with_context(|| format!("failed to open {}", path.display()))?;

    let pack_id = pack.manifest.meta.pack_id.clone();
    let mut flow_cache: HashMap<String, Flow> = HashMap::new();
    let mut flows = Vec::new();
    let mut registered: HashSet<String> = HashSet::new();

    for entry in &pack.manifest.flows {
        let yaml = pack
            .files
            .get(&entry.file_yaml)
            .ok_or_else(|| anyhow!("missing flow file {}", entry.file_yaml))?;
        let contents = String::from_utf8(yaml.clone())
            .with_context(|| format!("flow file {} is not UTF-8", entry.file_yaml))?;
        let flow = Flow::load_from_str(&entry.file_yaml, &contents)?;
        flow_cache.insert(entry.id.clone(), flow);
    }

    if let Some(messaging) = pack.manifest.meta.messaging.as_ref()
        && let Some(adapters) = messaging.adapters.as_ref()
    {
        for adapter in adapters {
            if let Some(flow_id) = resolve_flow_id_for_adapter(adapter, &pack.manifest.flows)
                && let Some(flow) = flow_cache.get(&flow_id).cloned()
            {
                flows.push(flow_definition_from_adapter(
                    pack_id.clone(),
                    adapter,
                    flow_id.clone(),
                    flow,
                ));
                registered.insert(flow_id);
            }
        }
    }

    for (flow_id, flow) in flow_cache {
        if registered.contains(&flow_id) {
            continue;
        }
        flows.push(FlowDefinition {
            pack_id: pack_id.clone(),
            flow_id,
            platform: None,
            route: None,
            flow,
        });
    }

    Ok(flows)
}

fn resolve_flow_id_for_adapter(
    adapter: &MessagingAdapter,
    flows: &[greentic_pack::builder::FlowEntry],
) -> Option<String> {
    let flow_path = adapter
        .custom_flow
        .as_ref()
        .or(adapter.default_flow.as_ref())?;
    flows
        .iter()
        .find(|entry| entry.file_yaml == *flow_path)
        .map(|entry| entry.id.clone())
}

fn resolve_flow_path(root: &Path, pack_dir: &Path, path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        bail!("absolute flow paths are not allowed: {}", path.display());
    }
    let joined = pack_dir.join(path);
    let canon = joined
        .canonicalize()
        .with_context(|| format!("failed to canonicalize {}", joined.display()))?;
    if !canon.starts_with(root) {
        bail!(
            "flow path escapes packs root ({}): {}",
            root.display(),
            canon.display()
        );
    }
    Ok(canon)
}

fn flow_definition_from_adapter(
    pack_id: String,
    adapter: &MessagingAdapter,
    flow_id: String,
    flow: Flow,
) -> FlowDefinition {
    let platform = infer_platform_from_adapter_name(&adapter.name)
        .map(|platform| platform.as_str().to_string());
    FlowDefinition {
        pack_id,
        flow_id,
        platform,
        route: Some(adapter.name.clone()),
        flow,
    }
}

fn infer_platform_from_adapter_name(name: &str) -> Option<Platform> {
    gsm_core::infer_platform_from_adapter_name(name)
}

#[derive(Debug, Deserialize)]
struct PackSpec {
    id: String,
    #[serde(default)]
    messaging: Option<greentic_pack::messaging::MessagingSection>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Flow {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(rename = "in")]
    pub r#in: String,
    pub nodes: BTreeMap<String, Node>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Node {
    #[serde(default)]
    pub qa: Option<QaNode>,
    #[serde(default)]
    pub tool: Option<ToolNode>,
    #[serde(default)]
    pub template: Option<TemplateNode>,
    #[serde(default)]
    pub card: Option<CardNode>,
    #[serde(default)]
    pub routes: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct QaNode {
    pub questions: Vec<Question>,
    #[serde(default)]
    pub fallback_agent: Option<AgentCfg>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Question {
    pub id: String,
    #[serde(default)]
    pub answer_type: Option<String>,
    #[serde(default)]
    pub max_words: Option<usize>,
    #[serde(default)]
    pub default: Option<Value>,
    #[serde(default)]
    pub validate: Option<Validate>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Validate {
    pub range: Option<[f64; 2]>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct AgentCfg {
    #[serde(default)]
    pub r#type: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    pub task: Option<String>,
    #[serde(default)]
    pub endpoint: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct ToolNode {
    pub tool: String,
    pub action: String,
    #[serde(default)]
    pub input: Value,
    #[serde(default)]
    pub retry: Option<u32>,
    #[serde(default)]
    pub delay_secs: Option<u64>,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TemplateNode {
    pub template: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CardNode {
    pub title: Option<String>,
    #[serde(default)]
    pub body: Vec<CardBlock>,
    #[serde(default)]
    pub actions: Vec<CardAction>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum CardBlock {
    #[serde(rename = "text")]
    Text {
        text: String,
        #[serde(default)]
        markdown: Option<bool>,
    },
    #[serde(rename = "fact")]
    Fact { label: String, value: String },
    #[serde(rename = "image")]
    Image { url: String },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum CardAction {
    #[serde(rename = "openUrl")]
    OpenUrl {
        title: String,
        url: String,
        #[serde(default)]
        jwt: Option<bool>,
    },
    #[serde(rename = "postback")]
    Postback { title: String, data: Value },
}

impl Flow {
    pub fn load_from_file(path: &str) -> anyhow::Result<Self> {
        let txt = fs::read_to_string(path)
            .with_context(|| format!("reading flow definition at {path}"))?;
        let flow: Flow =
            serde_yaml::from_str(&txt).with_context(|| format!("parsing flow yaml at {path}"))?;
        flow.validate()?;
        Ok(flow)
    }

    pub fn load_from_str(label: &str, raw: &str) -> anyhow::Result<Self> {
        let flow: Flow =
            serde_yaml::from_str(raw).with_context(|| format!("parsing flow yaml at {label}"))?;
        flow.validate()?;
        Ok(flow)
    }

    fn validate(&self) -> anyhow::Result<()> {
        if self.id.trim().is_empty() {
            bail!("flow missing id");
        }
        if self.kind.trim().is_empty() {
            bail!("flow {} missing type", self.id);
        }
        if self.r#in.trim().is_empty() {
            bail!("flow {} missing entry point `in`", self.id);
        }
        if self.nodes.is_empty() {
            bail!("flow {} defines no nodes", self.id);
        }
        if !self.nodes.contains_key(&self.r#in) {
            bail!(
                "flow {} entry point `{}` not found in nodes",
                self.id,
                self.r#in
            );
        }
        Ok(())
    }
}

fn run_flow(flow: &Flow, env: &MessageEnvelope) -> Result<FlowRunOutcome> {
    let hbs = hb_registry();
    let mut state = json!({});
    let mut payload = json!({});
    let mut current = flow.r#in.clone();
    let mut visited: HashSet<String> = HashSet::new();
    let max_steps = flow.nodes.len().saturating_mul(4).max(1);
    let mut out_messages = Vec::new();
    let mut tool_calls = Vec::new();

    for _ in 0..max_steps {
        if !visited.insert(current.clone()) {
            bail!("detected cycle at node {}", current);
        }
        let node = flow
            .nodes
            .get(&current)
            .ok_or_else(|| anyhow!("node not found: {}", current))?;

        if let Some(qa) = &node.qa {
            run_qa_offline(qa, env, &mut state)?;
        }

        if let Some(tool) = &node.tool {
            let (output, call) = run_tool_stub(tool, env, &state)?;
            tool_calls.push(call);
            payload = output;
        }

        if let Some(tpl) = &node.template {
            let out = render_template(tpl, &hbs, env, &state, &payload)?;
            out_messages.push(OutMessage {
                ctx: make_tenant_ctx(env.tenant.clone(), None, Some(env.user_id.clone())),
                tenant: env.tenant.clone(),
                platform: env.platform.clone(),
                chat_id: env.chat_id.clone(),
                thread_id: env.thread_id.clone(),
                kind: OutKind::Text,
                text: Some(out),
                message_card: None,
                adaptive_card: None,
                meta: Default::default(),
            });
        }

        if let Some(card) = &node.card {
            let card = render_card(card, &hbs, env, &state, &payload)?;
            out_messages.push(OutMessage {
                ctx: make_tenant_ctx(env.tenant.clone(), None, Some(env.user_id.clone())),
                tenant: env.tenant.clone(),
                platform: env.platform.clone(),
                chat_id: env.chat_id.clone(),
                thread_id: env.thread_id.clone(),
                kind: OutKind::Card,
                text: None,
                message_card: Some(card),
                adaptive_card: None,
                meta: Default::default(),
            });
        }

        if let Some(next) = node.routes.first() {
            if next == "end" {
                break;
            }
            current = next.clone();
            continue;
        }
        break;
    }

    Ok(FlowRunOutcome {
        out_messages,
        tool_calls,
    })
}

fn hb_registry() -> Handlebars<'static> {
    let mut h = Handlebars::new();
    h.set_strict_mode(true);
    h
}

fn render_template(
    tpl: &TemplateNode,
    hbs: &Handlebars<'static>,
    env: &MessageEnvelope,
    state: &Value,
    payload: &Value,
) -> Result<String> {
    let ctx = json!({
      "envelope": env,
      "state": state,
      "payload": payload
    });
    Ok(hbs.render_template(&tpl.template, &ctx)?)
}

fn render_card(
    card: &CardNode,
    hbs: &Handlebars<'static>,
    env: &MessageEnvelope,
    state: &Value,
    payload: &Value,
) -> Result<gsm_core::MessageCard> {
    let mut title = None;
    if let Some(t) = &card.title {
        title = Some(hbs.render_template(
            t,
            &json!({"envelope":env, "state":state, "payload":payload}),
        )?);
    }
    let mut body = vec![];
    for b in &card.body {
        match b {
            CardBlock::Text { text, markdown } => {
                body.push(gsm_core::CardBlock::Text {
                    text: hbs.render_template(
                        text,
                        &json!({"envelope":env, "state":state, "payload":payload}),
                    )?,
                    markdown: markdown.unwrap_or(true),
                });
            }
            CardBlock::Fact { label, value } => {
                body.push(gsm_core::CardBlock::Fact {
                    label: hbs.render_template(
                        label,
                        &json!({"envelope":env, "state":state, "payload":payload}),
                    )?,
                    value: hbs.render_template(
                        value,
                        &json!({"envelope":env, "state":state, "payload":payload}),
                    )?,
                });
            }
            CardBlock::Image { url } => {
                body.push(gsm_core::CardBlock::Image {
                    url: hbs.render_template(
                        url,
                        &json!({"envelope":env, "state":state, "payload":payload}),
                    )?,
                });
            }
        }
    }
    let mut actions = vec![];
    for a in &card.actions {
        match a {
            CardAction::OpenUrl { title, url, jwt } => {
                actions.push(gsm_core::CardAction::OpenUrl {
                    title: hbs.render_template(
                        title,
                        &json!({"envelope":env, "state":state, "payload":payload}),
                    )?,
                    url: hbs.render_template(
                        url,
                        &json!({"envelope":env, "state":state, "payload":payload}),
                    )?,
                    jwt: jwt.unwrap_or(false),
                })
            }
            CardAction::Postback { title, data } => {
                let title = hbs.render_template(
                    title,
                    &json!({"envelope":env, "state":state, "payload":payload}),
                )?;
                let data_json = json!(data);
                actions.push(gsm_core::CardAction::Postback {
                    title,
                    data: data_json,
                });
            }
        }
    }
    Ok(gsm_core::MessageCard {
        title,
        body,
        actions,
    })
}

fn run_qa_offline(cfg: &QaNode, env: &MessageEnvelope, state: &mut Value) -> Result<()> {
    if !state.is_object() {
        *state = json!({});
    }
    let obj = state.as_object_mut().unwrap();

    for q in &cfg.questions {
        if !obj.contains_key(&q.id)
            && let Some(def) = &q.default
        {
            obj.insert(q.id.clone(), def.clone());
        }
    }

    let mut missing: Vec<&str> = cfg
        .questions
        .iter()
        .filter(|q| !obj.contains_key(&q.id))
        .map(|q| q.id.as_str())
        .collect();

    if !missing.is_empty()
        && let Some(text) = &env.text
    {
        let number_re = Regex::new(r"(?P<n>\\d+)").unwrap();
        for q in &cfg.questions {
            if missing.contains(&q.id.as_str()) {
                match q.answer_type.as_deref() {
                    Some("number") => {
                        if let Some(caps) = number_re.captures(text)
                            && let Some(m) = caps.name("n")
                        {
                            obj.insert(q.id.clone(), json!(m.as_str().parse::<i64>().unwrap_or(1)));
                        }
                    }
                    _ => {
                        let loc = text
                            .split_whitespace()
                            .take(q.max_words.unwrap_or(3))
                            .collect::<Vec<_>>()
                            .join(" ");
                        if !loc.is_empty() {
                            obj.insert(q.id.clone(), json!(loc));
                        }
                    }
                }
            }
        }
        missing = cfg
            .questions
            .iter()
            .filter(|q| !obj.contains_key(&q.id))
            .map(|q| q.id.as_str())
            .collect();
    }

    if !missing.is_empty() && cfg.fallback_agent.is_some() {
        bail!("qa fallback agent requires network; offline conformance cannot continue");
    }

    for q in &cfg.questions {
        if let Some(val) = obj.get(&q.id).cloned() {
            if let Some(r) = q.validate.as_ref().and_then(|v| v.range) {
                let n = val
                    .as_f64()
                    .or_else(|| val.as_i64().map(|x| x as f64))
                    .unwrap_or(0.0);
                let clamped = n.clamp(r[0], r[1]);
                obj.insert(q.id.clone(), json!(clamped));
            }
            if let Some(maxw) = q.max_words {
                let s = val.as_str().unwrap_or_default();
                if s.split_whitespace().count() > maxw {
                    bail!("answer '{}' exceeds max_words {}", q.id, maxw);
                }
            }
        }
    }
    Ok(())
}

fn run_tool_stub(
    cfg: &ToolNode,
    env: &MessageEnvelope,
    state: &Value,
) -> Result<(Value, ToolCall)> {
    let mut input = cfg.input.clone();
    render_json_strings(&mut input, &json!({"state":state, "envelope":env}))?;
    let call = ToolCall {
        tool: cfg.tool.clone(),
        action: cfg.action.clone(),
        input: input.clone(),
    };
    Ok((json!({"ok": true}), call))
}

fn render_json_strings(value: &mut Value, ctx: &Value) -> Result<()> {
    let h = Handlebars::new();
    match value {
        Value::String(s) => {
            *s = h.render_template(s, ctx)?;
        }
        Value::Array(arr) => {
            for v in arr {
                render_json_strings(v, ctx)?;
            }
        }
        Value::Object(map) => {
            for (_, v) in map.iter_mut() {
                render_json_strings(v, ctx)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn load_override_packs(paths: &[PathBuf]) -> Vec<DiscoveredPack> {
    let mut packs = Vec::new();
    for path in paths {
        let manifest = match load_pack_manifest(path) {
            Ok(manifest) => manifest,
            Err(err) => {
                packs.push(DiscoveredPack {
                    path: path.clone(),
                    manifest: None,
                    error: Some(err.to_string()),
                });
                continue;
            }
        };
        packs.push(DiscoveredPack {
            path: path.clone(),
            manifest,
            error: None,
        });
    }
    packs
}

fn load_pack_manifest(path: &Path) -> Result<Option<PackManifest>> {
    if path.extension().and_then(|s| s.to_str()) != Some("gtpack") {
        return Ok(None);
    }
    let file = fs::File::open(path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    let mut buf = Vec::new();
    archive
        .by_name("manifest.cbor")
        .context("manifest.cbor missing")?
        .read_to_end(&mut buf)
        .context("read manifest.cbor")?;
    let manifest = greentic_types::decode_pack_manifest(&buf).context("decode manifest")?;
    Ok(Some(manifest))
}
