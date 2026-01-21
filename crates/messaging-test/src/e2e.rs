use std::collections::BTreeMap;
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::runtime::Runtime;

use axum::http::HeaderMap;
use greentic_types::{PackId, ProviderInstallId, ProviderInstallRecord};
use gsm_bus::{BusClient, InMemoryBusClient};
use gsm_core::infer_platform_from_adapter_name;
use gsm_core::{
    AdapterRegistry, EnvId, InMemoryProviderInstallStore, Platform, ProviderExtensionsRegistry,
    ProviderInstallState, ProviderInstallStore, TenantCtx,
    load_provider_extensions_from_pack_files, make_tenant_ctx,
};
use gsm_core::{HttpRunnerClient, LoggingRunnerClient, OutMessage};
use gsm_egress::adapter_registry::AdapterLookup;
use gsm_egress::config::EgressConfig;
use gsm_egress::process_message_internal;
use gsm_gateway::config::GatewayConfig;
use gsm_gateway::http::{GatewayState, NormalizedRequest, handle_ingress};
use gsm_runner::engine::{ExecutionOptions, RunnerOutcome, RunnerSink, ToolMode, run_flow};
use gsm_runner::flow_registry::FlowRegistry;
use gsm_runner::model::{Flow, Node, TemplateNode};
use gsm_runner::template_node::hb_registry;
use gsm_session::shared_memory_store;
use gsm_telemetry::set_current_tenant_ctx;
use semver::Version;
use time::OffsetDateTime;

use crate::cli::PackDiscoveryArgs;
use crate::packs::{DiscoveredPack, discover_packs};

#[derive(Debug, Clone)]
pub struct E2eOptions {
    pub packs_dir: PathBuf,
    pub provider: Option<String>,
    pub report: Option<PathBuf>,
    pub dry_run: bool,
    pub live: bool,
    pub trace: bool,
}

#[derive(Debug, Serialize)]
struct E2eAggregateReport {
    summary: E2eSummary,
    reports: Vec<E2ePackReport>,
}

#[derive(Debug, Serialize)]
struct E2eSummary {
    total: usize,
    passed: usize,
    failed: usize,
}

#[derive(Debug, Serialize)]
struct E2ePackReport {
    pack: String,
    version: String,
    status: String,
    stages: E2eStageReport,
    diagnostics: Vec<String>,
    timing_ms: BTreeMap<String, u128>,
}

#[derive(Debug, Serialize)]
struct E2eStageReport {
    requirements: String,
    setup: String,
    ingress: String,
    runner: String,
    egress: String,
    subscriptions: String,
}

#[derive(Clone)]
struct CollectingSink {
    egress_prefix: String,
    published: InMemoryBusClient,
}

#[async_trait]
impl RunnerSink for CollectingSink {
    async fn publish_out_message(&self, subject: &str, out: &OutMessage) -> Result<()> {
        let payload = serde_json::to_value(out)?;
        let subject = if subject.is_empty() {
            format!("{}.local", self.egress_prefix)
        } else {
            subject.to_string()
        };
        self.published.publish_value(&subject, payload).await?;
        Ok(())
    }
}

pub fn run_e2e(options: E2eOptions) -> Result<()> {
    if !options.dry_run && !options.live {
        bail!("--dry-run=false requires --live for safety");
    }
    if options.live {
        ensure_live_env()?;
    }
    ensure_provision_available()?;

    if options.trace {
        let _ = tracing_subscriber::fmt()
            .with_env_filter("trace")
            .try_init();
    }

    let discovery = PackDiscoveryArgs {
        roots: vec![options.packs_dir.clone()],
        glob: "messaging-*.gtpack".into(),
    };
    let mut packs = discover_packs(&discovery)?;
    if packs.is_empty() {
        bail!(
            "no packs matched messaging-*.gtpack in {}",
            options.packs_dir.display()
        );
    }

    if let Some(filter) = options.provider.as_ref() {
        packs.retain(|pack| pack_matches_filter(pack, filter));
    }
    if packs.is_empty() {
        bail!("no packs matched provider filter");
    }

    let runtime = Runtime::new().context("build e2e runtime")?;
    let effective_dry = if options.live { false } else { options.dry_run };

    let mut reports = Vec::new();
    for pack in &packs {
        let report = run_pack_e2e(&runtime, pack, effective_dry, options.live)?;
        reports.push(report);
    }

    let summary = summarize(&reports);
    let aggregate = E2eAggregateReport { summary, reports };

    if let Some(path) = options.report.as_ref() {
        let payload = serde_json::to_string_pretty(&aggregate)?;
        std::fs::write(path, payload)
            .with_context(|| format!("write report {}", path.display()))?;
    }

    let failures = aggregate
        .reports
        .iter()
        .filter(|r| r.status == "fail")
        .count();
    if failures > 0 {
        Err(anyhow!("{failures} pack(s) failed e2e conformance"))
    } else {
        Ok(())
    }
}

fn summarize(reports: &[E2ePackReport]) -> E2eSummary {
    let total = reports.len();
    let passed = reports.iter().filter(|r| r.status == "pass").count();
    let failed = total - passed;
    E2eSummary {
        total,
        passed,
        failed,
    }
}

fn pack_matches_filter(pack: &DiscoveredPack, filter: &str) -> bool {
    let filter = filter.to_ascii_lowercase();
    if let Some(manifest) = pack.manifest.as_ref()
        && manifest
            .pack_id
            .as_str()
            .to_ascii_lowercase()
            .contains(&filter)
    {
        return true;
    }
    pack.path
        .file_name()
        .and_then(|s| s.to_str())
        .is_some_and(|name| name.to_ascii_lowercase().contains(&filter))
}

fn run_pack_e2e(
    runtime: &Runtime,
    pack: &DiscoveredPack,
    dry_run: bool,
    live: bool,
) -> Result<E2ePackReport> {
    let started = Instant::now();
    let mut diagnostics = Vec::new();
    let mut stages = E2eStageReport {
        requirements: "skip".into(),
        setup: "skip".into(),
        ingress: "skip".into(),
        runner: "skip".into(),
        egress: "skip".into(),
        subscriptions: "skip".into(),
    };

    let Some(manifest) = pack.manifest.as_ref() else {
        diagnostics.push("pack manifest unavailable".to_string());
        let pack_id = pack.path.display().to_string();
        return Ok(report_for_pack(
            pack,
            pack_id.as_str(),
            "0.0.0",
            "fail",
            stages,
            diagnostics,
            started,
        ));
    };
    if let Some(err) = pack.error.as_ref() {
        diagnostics.push(format!("pack load error: {err}"));
        let version = manifest.version.to_string();
        let pack_id = manifest.pack_id.to_string();
        return Ok(report_for_pack(
            pack,
            pack_id.as_str(),
            version.as_str(),
            "fail",
            stages,
            diagnostics,
            started,
        ));
    }

    let pack_root = pack
        .path
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let pack_paths = vec![pack.path.clone()];
    let provider_extensions = load_provider_extensions_from_pack_files(&pack_root, &pack_paths)?;
    if provider_extensions.is_empty() {
        diagnostics.push("no messaging provider extensions found".to_string());
        stages.requirements = "fail".into();
        let version = manifest.version.to_string();
        let pack_id = manifest.pack_id.to_string();
        return Ok(report_for_pack(
            pack,
            pack_id.as_str(),
            version.as_str(),
            "fail",
            stages,
            diagnostics,
            started,
        ));
    }

    diagnostics.push(format!(
        "oauth required: {}",
        if provider_extensions.oauth.is_empty() {
            "no"
        } else {
            "yes"
        }
    ));
    diagnostics.push(format!(
        "subscriptions required: {}",
        if provider_extensions.subscriptions.is_empty() {
            "no"
        } else {
            "yes"
        }
    ));

    let adapters =
        AdapterRegistry::load_from_paths(&pack_root, &pack_paths).context("load pack adapters")?;
    let flows =
        FlowRegistry::load_from_paths(&pack_root, &pack_paths).context("load pack flows")?;
    let base_req =
        load_ingress_fixture(Path::new("fixtures/ingress.request.json")).unwrap_or_else(|err| {
            diagnostics.push(format!("ingress fixture missing: {err}"));
            NormalizedRequest::default()
        });
    let setup_input =
        load_setup_fixture(Path::new("fixtures/setup.input.json")).unwrap_or_else(|err| {
            diagnostics.push(format!("setup fixture missing: {err}"));
            json!({})
        });

    let tenant_ctx = make_tenant_ctx("dev".into(), Some("default".into()), Some("user".into()));
    set_current_tenant_ctx(tenant_ctx.clone());

    let base_env = default_envelope(&tenant_ctx, &base_req, &adapters);
    let provider_id = match primary_provider_id(manifest, &provider_extensions) {
        Some(provider_id) => provider_id,
        None => {
            diagnostics.push("pack has no provider id".to_string());
            let version = manifest.version.to_string();
            let pack_id = manifest.pack_id.to_string();
            return Ok(report_for_pack(
                pack,
                pack_id.as_str(),
                version.as_str(),
                "fail",
                stages,
                diagnostics,
                started,
            ));
        }
    };

    let install_id = ProviderInstallId::new("e2e-install").expect("static install id");
    let ingress_channel = adapters
        .all()
        .into_iter()
        .find(|adapter| adapter.allows_ingress())
        .map(|adapter| adapter.name.clone())
        .unwrap_or_else(|| "slack".to_string());
    let provider_channel_id = base_req
        .provider_channel_id
        .clone()
        .unwrap_or_else(|| "channel-1".to_string());
    let install_state = build_install_state(
        &tenant_ctx,
        &provider_id,
        &install_id,
        &manifest.pack_id,
        &manifest.version,
        &provider_channel_id,
        &ingress_channel,
        &provider_extensions,
    );

    stages.requirements = match run_requirements_stage(
        runtime,
        manifest,
        &flows,
        &tenant_ctx,
        &base_env,
        &setup_input,
        &mut diagnostics,
        dry_run,
        live,
        &pack.path,
    ) {
        Ok(status) => status,
        Err(err) => {
            diagnostics.push(format!("requirements failed: {err}"));
            "fail".into()
        }
    };

    stages.setup = match run_setup_stage(
        runtime,
        manifest,
        &flows,
        &tenant_ctx,
        &base_env,
        &setup_input,
        &mut diagnostics,
        dry_run,
        live,
        &pack.path,
        &provider_id,
        &install_id,
        &provider_extensions,
    ) {
        Ok(status) => status,
        Err(err) => {
            diagnostics.push(format!("setup failed: {err}"));
            "fail".into()
        }
    };

    let ingress_result = match run_ingress_stage(
        runtime,
        &provider_extensions,
        &adapters,
        &tenant_ctx,
        &base_req,
        &provider_id,
        &install_state,
        &mut diagnostics,
    ) {
        Ok(outcome) => outcome,
        Err(err) => {
            diagnostics.push(format!("ingress failed: {err}"));
            IngressOutcome::fail()
        }
    };
    stages.ingress = ingress_result.status.clone();

    let runner_env = ingress_result
        .channel
        .as_ref()
        .and_then(|env| gsm_runner::engine::env_from_channel(env).ok())
        .unwrap_or_else(|| base_env.clone());

    stages.runner = match run_runner_stage(
        runtime,
        manifest,
        &flows,
        &tenant_ctx,
        &runner_env,
        &mut diagnostics,
        dry_run,
        live,
    ) {
        Ok(status) => status,
        Err(err) => {
            diagnostics.push(format!("runner failed: {err}"));
            "fail".into()
        }
    };

    stages.egress = match run_egress_stage(
        runtime,
        &adapters,
        &tenant_ctx,
        &install_state,
        &mut diagnostics,
        dry_run,
        live,
    ) {
        Ok(status) => status,
        Err(err) => {
            diagnostics.push(format!("egress failed: {err:?}"));
            "fail".into()
        }
    };

    stages.subscriptions = match run_subscriptions_stage(
        runtime,
        manifest,
        &flows,
        &tenant_ctx,
        &base_env,
        &setup_input,
        &mut diagnostics,
        dry_run,
        live,
        &provider_extensions,
        &pack.path,
        &provider_id,
        &install_id,
    ) {
        Ok(status) => status,
        Err(err) => {
            diagnostics.push(format!("subscriptions failed: {err}"));
            "fail".into()
        }
    };

    let status = if stages.requirements == "fail"
        || stages.setup == "fail"
        || stages.ingress == "fail"
        || stages.runner == "fail"
        || stages.egress == "fail"
        || stages.subscriptions == "fail"
    {
        "fail"
    } else {
        "pass"
    };

    let version = manifest.version.to_string();
    let pack_id = manifest.pack_id.to_string();
    Ok(report_for_pack(
        pack,
        pack_id.as_str(),
        version.as_str(),
        status,
        stages,
        diagnostics,
        started,
    ))
}

fn report_for_pack(
    _pack: &DiscoveredPack,
    pack_id: &str,
    version: &str,
    status: &str,
    stages: E2eStageReport,
    diagnostics: Vec<String>,
    started: Instant,
) -> E2ePackReport {
    let mut timing_ms = BTreeMap::new();
    timing_ms.insert("total".to_string(), started.elapsed().as_millis());
    E2ePackReport {
        pack: pack_id.to_string(),
        version: version.to_string(),
        status: status.to_string(),
        stages,
        diagnostics,
        timing_ms,
    }
}

#[allow(clippy::too_many_arguments)]
fn run_requirements_stage(
    runtime: &Runtime,
    manifest: &greentic_types::pack_manifest::PackManifest,
    flows: &FlowRegistry,
    tenant_ctx: &TenantCtx,
    env: &gsm_core::MessageEnvelope,
    setup_input: &Value,
    diagnostics: &mut Vec<String>,
    dry_run: bool,
    live: bool,
    pack_path: &Path,
) -> Result<String> {
    let flow_id = requirements_flow_id(manifest);
    if let Some(flow_id) = flow_id {
        if let Some(flow) = flows.get_flow(&flow_id) {
            let (status, outcome) = run_flow_deterministic(
                runtime,
                &flow.flow_id,
                &flow.flow,
                tenant_ctx,
                env,
                setup_input,
                diagnostics,
                dry_run,
                live,
            )?;
            if status == "pass" {
                let outcome = outcome.context("requirements flow returned no outcome")?;
                if !outcome.state.is_object() {
                    diagnostics.push("requirements output is not an object".to_string());
                    return Ok("fail".into());
                }
                if !record_requirements_from_state(&outcome.state, diagnostics) {
                    infer_requirements(manifest, pack_path, setup_input, diagnostics)?;
                }
            }
            return Ok(status);
        }
        diagnostics.push(format!("requirements flow {flow_id} not found"));
        return Ok("fail".into());
    }

    infer_requirements(manifest, pack_path, setup_input, diagnostics)?;
    Ok("pass".into())
}

#[allow(clippy::too_many_arguments)]
fn run_setup_stage(
    runtime: &Runtime,
    manifest: &greentic_types::pack_manifest::PackManifest,
    flows: &FlowRegistry,
    tenant_ctx: &TenantCtx,
    env: &gsm_core::MessageEnvelope,
    setup_input: &Value,
    diagnostics: &mut Vec<String>,
    dry_run: bool,
    live: bool,
    pack_path: &Path,
    provider_id: &str,
    install_id: &ProviderInstallId,
    extensions: &ProviderExtensionsRegistry,
) -> Result<String> {
    let public_base_url = setup_input
        .get("public_base_url")
        .and_then(Value::as_str)
        .unwrap_or("https://example.invalid");
    let setup_plan = run_provision_deterministic(
        "setup",
        provider_id,
        install_id,
        pack_path,
        tenant_ctx,
        public_base_url,
        setup_input,
        diagnostics,
    )?;
    if extensions.ingress.contains_key(provider_id)
        && !plan_contains_keyword(&setup_plan, "webhook")
    {
        diagnostics.push("setup plan missing webhook operations".to_string());
        return Ok("fail".into());
    }
    if extensions.subscriptions.contains_key(provider_id)
        && !plan_contains_keyword(&setup_plan, "subscription")
    {
        diagnostics.push("setup plan missing subscription operations".to_string());
        return Ok("fail".into());
    }

    let flow_id = setup_flow_id(manifest);
    let Some(flow_id) = flow_id else {
        diagnostics.push("no setup flow declared".to_string());
        return Ok("skip".into());
    };
    let Some(flow) = flows.get_flow(&flow_id) else {
        diagnostics.push(format!("setup flow {flow_id} not found"));
        return Ok("fail".into());
    };

    let (status, outcome) = run_flow_deterministic(
        runtime,
        &flow.flow_id,
        &flow.flow,
        tenant_ctx,
        env,
        setup_input,
        diagnostics,
        dry_run,
        live,
    )?;
    if status == "pass"
        && let Some(outcome) = outcome
        && outcome.tool_calls.is_empty()
    {
        diagnostics.push("setup produced no tool calls".to_string());
    }
    Ok(status)
}

#[allow(clippy::too_many_arguments)]
fn run_ingress_stage(
    runtime: &Runtime,
    extensions: &ProviderExtensionsRegistry,
    adapters: &AdapterRegistry,
    tenant_ctx: &TenantCtx,
    request: &NormalizedRequest,
    provider_id: &str,
    install_state: &ProviderInstallState,
    diagnostics: &mut Vec<String>,
) -> Result<IngressOutcome> {
    if extensions.ingress.is_empty() {
        return Ok(IngressOutcome::skip());
    }

    let ingress_adapter = adapters
        .all()
        .into_iter()
        .find(|adapter| adapter.allows_ingress());
    let Some(adapter) = ingress_adapter else {
        diagnostics.push("ingress declared but no ingress adapters found".to_string());
        return Ok(IngressOutcome::fail());
    };

    let bus = InMemoryBusClient::default();
    let config = GatewayConfig {
        env: EnvId("dev".into()),
        nats_url: "nats://127.0.0.1:4222".into(),
        addr: "127.0.0.1:0".parse().unwrap(),
        default_team: "default".into(),
        subject_prefix: gsm_core::INGRESS_SUBJECT_PREFIX.to_string(),
        worker_routing: None,
        worker_routes: Default::default(),
        packs_root: PathBuf::from("."),
        default_packs: Default::default(),
        extra_pack_paths: Vec::new(),
        install_store_path: None,
    };
    let store = std::sync::Arc::new(InMemoryProviderInstallStore::default());
    store.insert(install_state.clone());
    let state = GatewayState {
        bus: std::sync::Arc::new(bus.clone()),
        config,
        adapters: adapters.clone(),
        provider_extensions: extensions.clone(),
        install_store: store,
        workers: Default::default(),
        worker_default: None,
    };

    let team = tenant_ctx.team.as_ref().map(|t| t.to_string());
    let channel = adapter.name.clone();
    let tenant = tenant_ctx.tenant.as_str().to_string();
    let mut req = request.clone();
    req.provider_id = Some(provider_id.to_string());
    req.provider_channel_id = req
        .provider_channel_id
        .clone()
        .or_else(|| Some("channel-1".to_string()));

    let headers = if extensions
        .ingress
        .get(provider_id)
        .map(|decl| decl.capabilities.supports_webhook_validation)
        .unwrap_or(false)
    {
        let mut headers = HeaderMap::new();
        let signature = sign_hmac_sha256(
            install_state
                .secrets
                .get("webhook_secret")
                .map(|s| s.as_str())
                .unwrap_or("secret"),
            &serde_json::to_vec(&req).unwrap_or_default(),
        );
        headers.insert("x-signature", signature.parse().unwrap());
        headers
    } else {
        HeaderMap::new()
    };

    let _ = runtime
        .block_on(async {
            tokio::time::timeout(
                Duration::from_secs(5),
                handle_ingress(
                    tenant,
                    team,
                    channel,
                    std::sync::Arc::new(state),
                    req,
                    headers,
                ),
            )
            .await
        })
        .context("ingress timeout")?
        .map_err(|err| anyhow!("ingress failed: {err:?}"))?;

    let published = runtime
        .block_on(async { bus.take_published().await })
        .into_iter()
        .collect::<Vec<_>>();
    if published.is_empty() {
        diagnostics.push("ingress published no messages".to_string());
        return Ok(IngressOutcome::fail());
    }
    let (_, value) = &published[0];
    let channel: gsm_core::ChannelMessage =
        serde_json::from_value(value.clone()).context("parse ingress ChannelMessage")?;
    if channel.tenant.tenant.as_str().is_empty() {
        diagnostics.push("ingress missing tenant in ChannelMessage".to_string());
    }
    if channel.channel_id.is_empty() {
        diagnostics.push("ingress missing channel in ChannelMessage".to_string());
    }
    if channel
        .payload
        .get("metadata")
        .and_then(|v| v.get("provider_id"))
        .is_none()
    {
        diagnostics.push("ingress missing provider metadata".to_string());
    }
    if channel
        .payload
        .get("metadata")
        .and_then(|v| v.get("adapter"))
        .is_none()
    {
        diagnostics.push("ingress missing provider adapter metadata".to_string());
    }
    Ok(IngressOutcome::success(channel))
}

#[allow(clippy::too_many_arguments)]
fn run_runner_stage(
    runtime: &Runtime,
    _manifest: &greentic_types::pack_manifest::PackManifest,
    flows: &FlowRegistry,
    tenant_ctx: &TenantCtx,
    env: &gsm_core::MessageEnvelope,
    diagnostics: &mut Vec<String>,
    dry_run: bool,
    live: bool,
) -> Result<String> {
    let flow = flows
        .select_flow(&stub_channel_message(tenant_ctx, env))
        .ok()
        .map(|f| (f.flow_id.clone(), f.flow.clone()))
        .unwrap_or_else(|| ("stub-flow".to_string(), stub_flow()));

    let status = run_flow_twice(
        runtime,
        &flow.0,
        &flow.1,
        tenant_ctx,
        env,
        &json!({}),
        diagnostics,
        dry_run,
        live,
    )?;
    Ok(status)
}

fn run_egress_stage(
    runtime: &Runtime,
    adapters: &AdapterRegistry,
    tenant_ctx: &TenantCtx,
    install_state: &ProviderInstallState,
    diagnostics: &mut Vec<String>,
    dry_run: bool,
    live: bool,
) -> Result<String> {
    let egress_adapter = adapters
        .all()
        .into_iter()
        .find(|adapter| adapter.allows_egress());
    let Some(adapter) = egress_adapter else {
        return Ok("skip".into());
    };

    let fixture = load_egress_fixture(Path::new("fixtures/egress.request.json"))
        .context("load egress fixture")?;
    let mut out = fixture;
    out.platform = infer_platform_from_adapter_name(&adapter.name).unwrap_or(out.platform);
    out.ctx = tenant_ctx.clone();
    out.tenant = tenant_ctx.tenant.to_string();

    let config = EgressConfig {
        env: tenant_ctx.env.clone(),
        nats_url: "nats://127.0.0.1:4222".into(),
        subject_filter: format!("{}.{}.>", gsm_core::EGRESS_SUBJECT_PREFIX, tenant_ctx.env.0),
        adapter: None,
        packs_root: ".".into(),
        egress_prefix: gsm_core::EGRESS_SUBJECT_PREFIX.to_string(),
        runner_http_url: live.then(runner_url_from_env),
        runner_http_api_key: None,
        install_store_path: None,
    };

    let bus = InMemoryBusClient::default();
    let runner: Box<dyn gsm_core::RunnerClient> = if dry_run {
        Box::new(LoggingRunnerClient)
    } else {
        let url = runner_url_from_env();
        Box::new(HttpRunnerClient::new(url, None)?)
    };

    let lookup = AdapterLookup::new(adapters);
    let resolved = lookup.egress(&adapter.name)?;

    runtime
        .block_on(async {
            tokio::time::timeout(
                Duration::from_secs(5),
                process_message_internal(
                    &out,
                    &resolved,
                    &bus,
                    runner.as_ref(),
                    &config,
                    install_state,
                ),
            )
            .await
        })
        .context("egress timeout")?
        .map_err(|err| {
            diagnostics.push(format!("egress failed: {err}"));
            anyhow!("egress failed")
        })?;

    let published = runtime.block_on(async { bus.take_published().await });
    if published.is_empty() {
        diagnostics.push("egress produced no plan output".to_string());
    }

    Ok("pass".into())
}

#[allow(clippy::too_many_arguments)]
fn run_subscriptions_stage(
    runtime: &Runtime,
    manifest: &greentic_types::pack_manifest::PackManifest,
    flows: &FlowRegistry,
    tenant_ctx: &TenantCtx,
    env: &gsm_core::MessageEnvelope,
    setup_input: &Value,
    diagnostics: &mut Vec<String>,
    dry_run: bool,
    live: bool,
    extensions: &ProviderExtensionsRegistry,
    pack_path: &Path,
    provider_id: &str,
    install_id: &ProviderInstallId,
) -> Result<String> {
    if extensions.subscriptions.is_empty() {
        return Ok("skip".into());
    }
    let public_base_url = setup_input
        .get("public_base_url")
        .and_then(Value::as_str)
        .unwrap_or("https://example.invalid");
    let plan = run_provision_deterministic(
        "sync-subscriptions",
        provider_id,
        install_id,
        pack_path,
        tenant_ctx,
        public_base_url,
        &json!({}),
        diagnostics,
    )?;
    if !plan_contains_keyword(&plan, "subscription") {
        diagnostics.push("subscriptions plan missing subscription operations".to_string());
        return Ok("fail".into());
    }

    let flow_id = subscriptions_flow_id(manifest);
    let Some(flow_id) = flow_id else {
        diagnostics.push("subscriptions declared but no flow found".to_string());
        return Ok("fail".into());
    };
    let Some(flow) = flows.get_flow(&flow_id) else {
        diagnostics.push(format!("subscriptions flow {flow_id} not found"));
        return Ok("fail".into());
    };

    let status = run_flow_twice(
        runtime,
        &flow.flow_id,
        &flow.flow,
        tenant_ctx,
        env,
        &json!({}),
        diagnostics,
        dry_run,
        live,
    )?;
    Ok(status)
}

#[allow(clippy::too_many_arguments)]
fn run_flow_twice(
    runtime: &Runtime,
    flow_id: &str,
    flow: &Flow,
    tenant_ctx: &TenantCtx,
    env: &gsm_core::MessageEnvelope,
    setup_input: &Value,
    diagnostics: &mut Vec<String>,
    dry_run: bool,
    live: bool,
) -> Result<String> {
    Ok(run_flow_deterministic(
        runtime,
        flow_id,
        flow,
        tenant_ctx,
        env,
        setup_input,
        diagnostics,
        dry_run,
        live,
    )?
    .0)
}

#[allow(clippy::too_many_arguments)]
fn run_flow_deterministic(
    runtime: &Runtime,
    flow_id: &str,
    flow: &Flow,
    tenant_ctx: &TenantCtx,
    env: &gsm_core::MessageEnvelope,
    setup_input: &Value,
    diagnostics: &mut Vec<String>,
    dry_run: bool,
    live: bool,
) -> Result<(String, Option<RunnerOutcome>)> {
    let hbs = hb_registry();
    let tool_mode = if dry_run {
        ToolMode::Stub
    } else {
        ToolMode::Live
    };
    let allow_agent = live;
    let options = ExecutionOptions {
        tool_mode,
        allow_agent,
        tool_endpoint: "http://localhost:18081".to_string(),
    };
    let sink = CollectingSink {
        egress_prefix: gsm_core::EGRESS_SUBJECT_PREFIX.to_string(),
        published: InMemoryBusClient::default(),
    };

    let mut env = env.clone();
    env.context
        .insert("setup_answers".into(), setup_input.clone());

    let sessions = shared_memory_store();
    let first = runtime.block_on(async {
        tokio::time::timeout(
            Duration::from_secs(5),
            run_flow(
                flow_id, flow, tenant_ctx, &env, &sessions, &hbs, &sink, &options, None,
            ),
        )
        .await
    });
    let first = match first {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(err)) => {
            diagnostics.push(format!("flow {flow_id} failed: {err}"));
            return Ok(("fail".into(), None));
        }
        Err(_) => {
            diagnostics.push(format!("flow {flow_id} timed out"));
            return Ok(("fail".into(), None));
        }
    };

    let sessions = shared_memory_store();
    let second = runtime.block_on(async {
        tokio::time::timeout(
            Duration::from_secs(5),
            run_flow(
                flow_id, flow, tenant_ctx, &env, &sessions, &hbs, &sink, &options, None,
            ),
        )
        .await
    });
    let second = match second {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(err)) => {
            diagnostics.push(format!("flow {flow_id} failed on repeat: {err}"));
            return Ok(("fail".into(), None));
        }
        Err(_) => {
            diagnostics.push(format!("flow {flow_id} timed out on repeat"));
            return Ok(("fail".into(), None));
        }
    };

    if !outcomes_match(&first, &second) {
        diagnostics.push(format!("flow {flow_id} output is nondeterministic"));
        return Ok(("fail".into(), Some(first)));
    }

    Ok(("pass".into(), Some(first)))
}

fn outcomes_match(a: &RunnerOutcome, b: &RunnerOutcome) -> bool {
    serde_json::to_value(&a.out_messages).ok() == serde_json::to_value(&b.out_messages).ok()
        && serde_json::to_value(&a.tool_calls).ok() == serde_json::to_value(&b.tool_calls).ok()
        && a.state == b.state
}

fn stub_channel_message(
    tenant_ctx: &TenantCtx,
    env: &gsm_core::MessageEnvelope,
) -> gsm_core::ChannelMessage {
    gsm_core::ChannelMessage {
        tenant: tenant_ctx.clone(),
        channel_id: env.platform.as_str().to_string(),
        session_id: env.chat_id.clone(),
        route: None,
        payload: json!({
            "chat_id": env.chat_id.clone(),
            "user_id": env.user_id.clone(),
            "thread_id": env.thread_id.clone(),
            "msg_id": env.msg_id.clone(),
            "text": env.text.clone(),
            "timestamp": env.timestamp.clone(),
            "metadata": env.context.clone(),
        }),
    }
}

fn stub_flow() -> Flow {
    let mut nodes = BTreeMap::new();
    nodes.insert(
        "start".into(),
        Node {
            qa: None,
            tool: None,
            template: Some(TemplateNode {
                template: "conformance stub".into(),
            }),
            card: None,
            routes: vec!["end".into()],
        },
    );
    Flow {
        id: "stub-flow".into(),
        title: Some("Stub Flow".into()),
        description: None,
        kind: "qa".into(),
        r#in: "start".into(),
        nodes,
    }
}

fn default_envelope(
    tenant_ctx: &TenantCtx,
    req: &NormalizedRequest,
    adapters: &AdapterRegistry,
) -> gsm_core::MessageEnvelope {
    let platform = adapters
        .all()
        .into_iter()
        .find_map(|adapter| infer_platform_from_adapter_name(&adapter.name))
        .unwrap_or(Platform::Slack);
    gsm_core::MessageEnvelope {
        tenant: tenant_ctx.tenant.as_str().to_string(),
        platform,
        chat_id: req.chat_id.clone().unwrap_or_else(|| "chat-1".into()),
        user_id: req.user_id.clone().unwrap_or_else(|| "user-1".into()),
        thread_id: req.thread_id.clone(),
        msg_id: req.msg_id.clone().unwrap_or_else(|| "msg-1".into()),
        text: req.text.clone(),
        timestamp: "2024-01-01T00:00:00Z".into(),
        context: {
            let mut ctx = BTreeMap::new();
            ctx.insert("source".into(), Value::String("e2e".into()));
            ctx
        },
    }
}

fn load_ingress_fixture(path: &Path) -> Result<NormalizedRequest> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("read ingress fixture {}", path.display()))?;
    let req: NormalizedRequest = serde_json::from_str(&raw)
        .with_context(|| format!("parse ingress fixture {}", path.display()))?;
    Ok(req)
}

fn load_setup_fixture(path: &Path) -> Result<Value> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("read setup fixture {}", path.display()))?;
    let value: Value = serde_json::from_str(&raw)
        .with_context(|| format!("parse setup fixture {}", path.display()))?;
    Ok(value)
}

fn load_egress_fixture(path: &Path) -> Result<OutMessage> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("read egress fixture {}", path.display()))?;
    let value: OutMessage = serde_json::from_str(&raw)
        .with_context(|| format!("parse egress fixture {}", path.display()))?;
    Ok(value)
}

fn requirements_flow_id(manifest: &greentic_types::pack_manifest::PackManifest) -> Option<String> {
    manifest
        .flows
        .iter()
        .find(|flow| {
            flow.entrypoints.iter().any(|id| id == "requirements")
                || flow.id.as_str().starts_with("requirements")
                || flow
                    .flow
                    .entrypoints
                    .values()
                    .any(|v| v.as_str() == Some("requirements"))
        })
        .map(|flow| flow.id.to_string())
}

fn setup_flow_id(manifest: &greentic_types::pack_manifest::PackManifest) -> Option<String> {
    manifest
        .flows
        .iter()
        .find(|flow| {
            flow.entrypoints.iter().any(|id| id == "setup")
                || flow.id.as_str().starts_with("setup")
                || flow
                    .flow
                    .entrypoints
                    .values()
                    .any(|v| v.as_str() == Some("setup"))
        })
        .map(|flow| flow.id.to_string())
}

fn subscriptions_flow_id(manifest: &greentic_types::pack_manifest::PackManifest) -> Option<String> {
    manifest
        .flows
        .iter()
        .find(|flow| {
            flow.entrypoints.iter().any(|id| id == "subscriptions")
                || flow.id.as_str().contains("subscriptions")
                || flow
                    .flow
                    .entrypoints
                    .values()
                    .any(|v| v.as_str() == Some("subscriptions"))
        })
        .map(|flow| flow.id.to_string())
}

fn infer_requirements(
    manifest: &greentic_types::pack_manifest::PackManifest,
    pack_path: &Path,
    setup_input: &Value,
    diagnostics: &mut Vec<String>,
) -> Result<()> {
    let mut required_configs = Vec::new();
    let mut required_keys = Vec::new();
    if let Some(ext) = manifest.extensions.as_ref()
        && let Some(entry) = ext.get(greentic_types::provider::PROVIDER_EXTENSION_ID)
        && let Some(inline) = entry.inline.as_ref()
        && let greentic_types::pack_manifest::ExtensionInline::Provider(provider_inline) = inline
    {
        for provider in &provider_inline.providers {
            let schema = provider.config_schema_ref.clone();
            required_configs.push(schema);
        }
    }
    if !required_configs.is_empty() {
        diagnostics.push(format!(
            "config schemas referenced: {}",
            required_configs.join(", ")
        ));
        if pack_path.extension().and_then(|s| s.to_str()) == Some("gtpack")
            && let Ok(pack) = greentic_pack::reader::open_pack(
                pack_path,
                greentic_pack::reader::SigningPolicy::DevOk,
            )
        {
            for schema_path in &required_configs {
                if let Some(raw) = pack.files.get(schema_path) {
                    if let Ok(schema) = serde_json::from_slice::<Value>(raw)
                        && let Some(req) = schema.get("required").and_then(|v| v.as_array())
                    {
                        for key in req.iter().filter_map(|v| v.as_str()) {
                            required_keys.push(key.to_string());
                        }
                    }
                } else {
                    diagnostics.push(format!("missing config schema {schema_path}"));
                }
            }
        }
    }
    if !required_keys.is_empty() {
        diagnostics.push(format!(
            "required config keys: {}",
            required_keys.join(", ")
        ));
    }

    if !manifest.secret_requirements.is_empty() {
        diagnostics.push(format!(
            "secret requirements: {}",
            manifest
                .secret_requirements
                .iter()
                .map(|req| format!("{req:?}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    if let Some(url) = setup_input.get("public_base_url").and_then(Value::as_str) {
        diagnostics.push(format!("public_base_url: {url}"));
    }
    Ok(())
}

fn ensure_live_env() -> Result<()> {
    let tests = env::var("RUN_LIVE_TESTS").unwrap_or_default();
    let http = env::var("RUN_LIVE_HTTP").unwrap_or_default();
    if tests != "true" || http != "true" {
        bail!("--live requires RUN_LIVE_TESTS=true and RUN_LIVE_HTTP=true");
    }
    Ok(())
}

fn runner_url_from_env() -> String {
    env::var("RUNNER_URL").unwrap_or_else(|_| "http://localhost:8081/invoke".into())
}

fn ensure_provision_available() -> Result<()> {
    let bin = env::var("GREENTIC_PROVISION_CLI").unwrap_or_else(|_| "greentic-provision".into());
    let candidate = PathBuf::from(&bin);
    let exists = if candidate.is_absolute() || candidate.components().count() > 1 {
        candidate.exists()
    } else {
        find_on_path(&bin).is_some()
    };
    if exists {
        Ok(())
    } else {
        bail!("greentic-provision binary not found (set GREENTIC_PROVISION_CLI or install it)")
    }
}

fn find_on_path(bin: &str) -> Option<PathBuf> {
    let path_var = env::var_os("PATH")?;
    for dir in env::split_paths(&path_var) {
        let candidate = dir.join(bin);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

fn primary_provider_id(
    manifest: &greentic_types::pack_manifest::PackManifest,
    extensions: &ProviderExtensionsRegistry,
) -> Option<String> {
    if let Some(exts) = manifest.extensions.as_ref()
        && let Some(entry) = exts.get(greentic_types::provider::PROVIDER_EXTENSION_ID)
        && let Some(inline) = entry.inline.as_ref()
        && let greentic_types::pack_manifest::ExtensionInline::Provider(provider_inline) = inline
        && let Some(provider) = provider_inline.providers.first()
    {
        return Some(provider.provider_type.clone());
    }
    extensions
        .ingress
        .keys()
        .next()
        .cloned()
        .or_else(|| extensions.subscriptions.keys().next().cloned())
}

#[allow(clippy::too_many_arguments)]
fn build_install_state(
    tenant_ctx: &TenantCtx,
    provider_id: &str,
    install_id: &ProviderInstallId,
    pack_id: &PackId,
    pack_version: &Version,
    provider_channel_id: &str,
    routing_platform: &str,
    extensions: &ProviderExtensionsRegistry,
) -> ProviderInstallState {
    let now = OffsetDateTime::now_utc();
    let mut record = ProviderInstallRecord {
        tenant: tenant_ctx.clone(),
        provider_id: provider_id.to_string(),
        install_id: install_id.clone(),
        pack_id: pack_id.clone(),
        pack_version: pack_version.clone(),
        created_at: now,
        updated_at: now,
        config_refs: BTreeMap::new(),
        secret_refs: BTreeMap::new(),
        webhook_state: serde_json::json!({}),
        subscriptions_state: serde_json::json!({}),
        metadata: serde_json::json!({
            "routing": {
                "platform": routing_platform,
                "channel_id": provider_channel_id,
            }
        }),
    };

    let mut state = ProviderInstallState::new(record.clone());
    if extensions
        .ingress
        .get(provider_id)
        .map(|decl| decl.capabilities.supports_webhook_validation)
        .unwrap_or(false)
    {
        record.webhook_state = serde_json::json!({
            "signature_header": "x-signature",
            "secret_key": "webhook_secret"
        });
        state.record = record;
        state
            .secrets
            .insert("webhook_secret".into(), "secret".into());
    }
    state
}

fn record_requirements_from_state(state: &Value, diagnostics: &mut Vec<String>) -> bool {
    let Some(obj) = state.as_object() else {
        return false;
    };
    let mut found = false;
    if let Some(keys) = obj.get("required_config_keys").and_then(Value::as_array) {
        let list = keys.iter().filter_map(Value::as_str).collect::<Vec<_>>();
        if !list.is_empty() {
            diagnostics.push(format!("required config keys: {}", list.join(", ")));
            found = true;
        }
    }
    if let Some(keys) = obj.get("required_secret_keys").and_then(Value::as_array) {
        let list = keys.iter().filter_map(Value::as_str).collect::<Vec<_>>();
        if !list.is_empty() {
            diagnostics.push(format!("required secret keys: {}", list.join(", ")));
            found = true;
        }
    }
    found
}

#[allow(clippy::too_many_arguments)]
fn run_provision_deterministic(
    action: &str,
    provider_id: &str,
    install_id: &ProviderInstallId,
    pack_path: &Path,
    tenant_ctx: &TenantCtx,
    public_base_url: &str,
    setup_input: &Value,
    diagnostics: &mut Vec<String>,
) -> Result<Value> {
    let first = run_provision_plan(
        action,
        provider_id,
        install_id,
        pack_path,
        tenant_ctx,
        public_base_url,
        setup_input,
    )?;
    let second = run_provision_plan(
        action,
        provider_id,
        install_id,
        pack_path,
        tenant_ctx,
        public_base_url,
        setup_input,
    )?;
    if first != second {
        diagnostics.push(format!("provision {action} output is nondeterministic"));
        bail!("provision {action} output is nondeterministic");
    }
    Ok(first)
}

fn run_provision_plan(
    action: &str,
    provider_id: &str,
    install_id: &ProviderInstallId,
    pack_path: &Path,
    tenant_ctx: &TenantCtx,
    public_base_url: &str,
    setup_input: &Value,
) -> Result<Value> {
    let bin = env::var("GREENTIC_PROVISION_CLI").unwrap_or_else(|_| "greentic-provision".into());
    let temp_home = tempfile::tempdir().context("create temp greentic home")?;
    let mut cmd = Command::new(bin);
    cmd.arg(action)
        .arg(provider_id)
        .arg("--install-id")
        .arg(install_id.to_string())
        .arg("--pack")
        .arg(pack_path)
        .arg("--env")
        .arg(tenant_ctx.env.as_str())
        .arg("--tenant")
        .arg(tenant_ctx.tenant.as_str())
        .arg("--public-base-url")
        .arg(public_base_url)
        .arg("--dry-run");
    cmd.env("GREENTIC_HOME", temp_home.path());
    cmd.env("GREENTIC_DISABLE_NETWORK", "true");
    cmd.env("GREENTIC_DISABLE_FS", "true");
    if let Some(team) = tenant_ctx.team.as_ref() {
        cmd.arg("--team").arg(team.as_str());
    }
    if let Some(answers) = setup_input.get("answers").and_then(Value::as_object)
        && !answers.is_empty()
    {
        cmd.arg("--answers")
            .arg(Value::Object(answers.clone()).to_string());
    }
    let output = cmd.output().context("run greentic-provision")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        bail!("greentic-provision {action} failed: {stderr} (stdout: {stdout})");
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let plan: Value = serde_json::from_str(&stdout)
        .with_context(|| format!("invalid provision plan JSON for {action}: {stdout}"))?;
    Ok(plan)
}

fn plan_contains_keyword(plan: &Value, keyword: &str) -> bool {
    match plan {
        Value::String(s) => s.to_ascii_lowercase().contains(keyword),
        Value::Array(values) => values.iter().any(|v| plan_contains_keyword(v, keyword)),
        Value::Object(map) => map.values().any(|v| plan_contains_keyword(v, keyword)),
        _ => false,
    }
}

fn sign_hmac_sha256(secret: &str, body: &[u8]) -> String {
    use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    type HmacSha256 = Hmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("hmac init");
    mac.update(body);
    B64.encode(mac.finalize().into_bytes())
}

#[derive(Debug, Deserialize, Clone)]
struct IngressOutcome {
    status: String,
    channel: Option<gsm_core::ChannelMessage>,
}

impl IngressOutcome {
    fn skip() -> Self {
        Self {
            status: "skip".into(),
            channel: None,
        }
    }

    fn fail() -> Self {
        Self {
            status: "fail".into(),
            channel: None,
        }
    }

    fn success(channel: gsm_core::ChannelMessage) -> Self {
        Self {
            status: "pass".into(),
            channel: Some(channel),
        }
    }
}
