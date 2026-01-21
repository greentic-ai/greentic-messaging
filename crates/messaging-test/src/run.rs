use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};
use tokio::runtime::Runtime;

use crate::adapters::{AdapterConfig, AdapterMode, AdapterTarget, registry_from_env};
use crate::cli::{Cli, CliCommand, PacksCommand};
use crate::conformance::{self, ConformanceReport, ConformanceStatus};
use crate::e2e;
use crate::fixtures::{Fixture, discover};
use crate::packs::{self, PackRunReport};
use greentic_types::{EnvId, TeamId, TenantCtx, TenantId};
use gsm_core::{
    AdapterDescriptor, AdapterRegistry, HttpRunnerClient, LoggingRunnerClient, OutKind, OutMessage,
    Platform, RunnerClient, infer_platform_from_adapter_name, messaging_card::MessageCardEngine,
};

struct PackAdapter {
    target: AdapterTarget,
    descriptor: AdapterDescriptor,
}

pub struct RunContext {
    cli: Cli,
    fixtures: Vec<Fixture>,
    config: AdapterConfig,
    engine: MessageCardEngine,
}

impl RunContext {
    pub fn new(cli: Cli) -> Result<Self> {
        let fixtures_dir = if cli.fixtures.exists() {
            cli.fixtures.clone()
        } else {
            PathBuf::from("crates/gsm-dev-viewer/fixtures")
        };
        let fixtures = discover(&fixtures_dir)?;
        let config = cli.adapter_config();
        let engine = MessageCardEngine::bootstrap();
        Ok(Self {
            cli,
            fixtures,
            config,
            engine,
        })
    }

    pub fn execute(self) -> Result<()> {
        match self.cli.command {
            CliCommand::List => self.list(),
            CliCommand::Fixtures => self.dump_fixtures(),
            CliCommand::Adapters => self.dump_adapters(),
            CliCommand::Run {
                ref fixture,
                dry_run,
            } => self.run_interactive(fixture, dry_run),
            CliCommand::All { dry_run } => self.run_all(dry_run),
            CliCommand::GenGolden => self.gen_golden(),
            CliCommand::Packs { ref command } => self.packs(command.as_ref()),
            CliCommand::E2e {
                packs,
                provider,
                report,
                dry_run,
                live,
                trace,
            } => e2e::run_e2e(e2e::E2eOptions {
                packs_dir: packs,
                provider,
                report,
                dry_run,
                live,
                trace,
            }),
        }
    }

    fn list(&self) -> Result<()> {
        for fixture in &self.fixtures {
            println!("{} -> {}", fixture.id, fixture.path.display());
        }
        Ok(())
    }

    fn dump_fixtures(&self) -> Result<()> {
        for fixture in &self.fixtures {
            println!("{}: {:?}", fixture.id, fixture.path);
        }
        Ok(())
    }

    fn dump_adapters(&self) -> Result<()> {
        let adapters = registry_from_env(self.config.mode);
        for adapter in adapters {
            println!(
                "{}: {}{}",
                adapter.name,
                if adapter.enabled {
                    "enabled"
                } else {
                    "disabled"
                },
                match &adapter.reason {
                    Some(reason) => format!(" ({reason})"),
                    None => "".into(),
                }
            );
        }
        Ok(())
    }

    fn adapter_mode(&self, override_dry: Option<bool>) -> AdapterMode {
        match override_dry {
            Some(true) => AdapterMode::DryRun,
            Some(false) => AdapterMode::Real,
            None => self.config.mode,
        }
    }

    fn run_interactive(&self, fixture_id: &str, dry_run: bool) -> Result<()> {
        if self.pack_mode_enabled() {
            return self.run_interactive_pack(fixture_id, dry_run);
        }
        let mut index = self.fixture_index(fixture_id)?;
        let mode = self.adapter_mode(Some(dry_run));
        let mut adapters = registry_from_env(mode);
        loop {
            let fixture = &self.fixtures[index];
            self.show_status(fixture, &adapters, index)?;
            let mut input = String::new();
            io::stdin().read_line(&mut input)?;
            match input.trim() {
                "" | "r" => self.process_fixture(fixture, &mut adapters)?,
                "n" => index = (index + 1) % self.fixtures.len(),
                "p" => {
                    index = if index == 0 {
                        self.fixtures.len() - 1
                    } else {
                        index - 1
                    }
                }
                "a" => self.toggle_adapters(&mut adapters)?,
                "q" => break,
                cmd => println!("Unknown command: {cmd}"),
            }
        }
        Ok(())
    }

    fn run_all(&self, dry_run: bool) -> Result<()> {
        if self.pack_mode_enabled() {
            return self.run_all_pack(dry_run);
        }
        if !dry_run {
            return Err(anyhow!("run all currently supports dry-run only"));
        }
        let mode = self.adapter_mode(Some(true));
        let mut adapters = registry_from_env(mode);
        for fixture in &self.fixtures {
            self.process_fixture(fixture, &mut adapters)?;
        }
        Ok(())
    }

    fn gen_golden(&self) -> Result<()> {
        let artifacts = PathBuf::from(".gsm-test/artifacts");
        if !artifacts.exists() {
            return Err(anyhow!("no artifacts to promote"));
        }
        let golden_root = PathBuf::from("crates/messaging-test/tests/golden");
        for fixture_dir in artifacts.read_dir().context("read artifacts dir")? {
            let fixture_dir = fixture_dir.context("artifact entry")?;
            if !fixture_dir.file_type()?.is_dir() {
                continue;
            }
            let fixture_name = fixture_dir.file_name();
            let fixture_root = golden_root.join(&fixture_name);
            for adapter_dir in fs::read_dir(fixture_dir.path()).context("read adapter dir")? {
                let adapter_dir = adapter_dir.context("adapter entry")?;
                if !adapter_dir.file_type()?.is_dir() {
                    continue;
                }
                let adapter_name = adapter_dir.file_name();
                let src = adapter_dir.path().join("translated.json");
                if !src.exists() {
                    continue;
                }
                let dst_dir = fixture_root.join(&adapter_name);
                fs::create_dir_all(&dst_dir)?;
                fs::copy(&src, dst_dir.join("translated.json"))
                    .context("copy translated payload to golden")?;
            }
        }
        println!("goldens updated");
        Ok(())
    }

    fn packs(&self, command: &PacksCommand) -> Result<()> {
        // Packs validation materializes components via distributor-client before linting.
        let materializer = packs::DistributorClientMaterializer::default();
        match command {
            PacksCommand::List { discovery } => {
                let packs = packs::discover_packs(discovery)?;
                if packs.is_empty() {
                    println!("No packs found under {:?}", discovery.roots);
                    return Ok(());
                }
                for pack in packs {
                    if let Some(err) = pack.error {
                        println!("{}: failed to load ({})", pack.path.display(), err);
                        continue;
                    }
                    let Some(manifest) = pack.manifest else {
                        continue;
                    };
                    println!(
                        "{}: {} {} (kind={:?})",
                        pack.path.display(),
                        manifest.pack_id,
                        manifest.version,
                        manifest.kind
                    );
                    if manifest.flows.is_empty() {
                        println!("  flows: (none)");
                    } else {
                        println!("  flows:");
                        for flow in &manifest.flows {
                            let title = flow.flow.metadata.title.clone().unwrap_or_default();
                            if title.is_empty() {
                                println!("    - {} (kind={:?})", flow.id, flow.kind);
                            } else {
                                println!("    - {} (kind={:?}) {title}", flow.id, flow.kind);
                            }
                        }
                    }
                }
                Ok(())
            }
            PacksCommand::Run {
                pack,
                discovery: _,
                runtime,
            } => {
                let report = packs::run_pack_from_path(pack, runtime, &materializer)?;
                self.print_pack_report(&report, runtime);
                if report.is_success() {
                    Ok(())
                } else {
                    Err(anyhow!("pack validation failed"))
                }
            }
            PacksCommand::All {
                discovery,
                runtime,
                fail_fast,
            } => {
                let discovered = packs::discover_packs(discovery)?;
                let reports =
                    packs::run_all_packs(&discovered, runtime, *fail_fast, &materializer)?;
                if reports.is_empty() {
                    println!("No packs matched {}", discovery.glob);
                    return Ok(());
                }
                let mut failures = 0;
                for report in reports {
                    self.print_pack_report(&report, runtime);
                    if !report.is_success() {
                        failures += 1;
                    }
                }
                if failures > 0 {
                    Err(anyhow!("{failures} pack(s) failed validation"))
                } else {
                    Ok(())
                }
            }
            PacksCommand::Conformance {
                discovery,
                runtime,
                public_base_url,
                ingress_fixture,
                pack,
                setup_only,
            } => {
                let reports = conformance::run_conformance(conformance::ConformanceOptions {
                    discovery: discovery.clone(),
                    runtime: runtime.clone(),
                    pack_paths: pack.clone(),
                    public_base_url: public_base_url.clone(),
                    ingress_fixture: ingress_fixture.clone(),
                    setup_only: *setup_only,
                })?;
                if reports.is_empty() {
                    println!("No packs matched {}", discovery.glob);
                    return Ok(());
                }
                let mut failures = 0;
                for report in &reports {
                    self.print_conformance_report(report);
                    if !report.is_success() {
                        failures += 1;
                    }
                }
                if failures > 0 {
                    Err(anyhow!("{failures} pack(s) failed conformance"))
                } else {
                    Ok(())
                }
            }
        }
    }

    fn print_pack_report(&self, report: &PackRunReport, runtime: &crate::cli::PackRuntimeArgs) {
        println!("Pack {} ({})", report.pack_id, report.pack_path.display());
        println!(
            "  flow: {} (kind={}){}",
            report.flow_id,
            report.flow_kind,
            if report.dry_run { " [dry-run]" } else { "" }
        );
        if !report.provider_ids.is_empty() {
            println!("  provider secrets:");
            for provider in &report.provider_ids {
                let uri = packs::format_secret_uri(
                    &runtime.env,
                    &runtime.tenant,
                    &runtime.team,
                    provider,
                );
                println!("    - {} -> {}", provider, packs::redact_secret_uri(&uri));
            }
        }
        if !report.secret_uris.is_empty() {
            println!("  required secrets:");
            for uri in &report.secret_uris {
                println!("    - {}", packs::redact_secret_uri(uri));
            }
        }
        if !report.steps.is_empty() {
            println!("  steps:");
            for step in &report.steps {
                let status = match &step.status {
                    packs::PackStepStatus::Planned => "planned".to_string(),
                    packs::PackStepStatus::Executed => "ok".to_string(),
                    packs::PackStepStatus::MissingComponent => "missing component".to_string(),
                };
                let op = step
                    .operation
                    .as_ref()
                    .map(|op| format!(" op={op}"))
                    .unwrap_or_default();
                println!(
                    "    - {} -> {}{op} [{status}]",
                    step.node_id, step.component_id
                );
            }
        }
        if !report.errors.is_empty()
            || !report.lint_errors.is_empty()
            || !report.missing_components.is_empty()
        {
            if !report.lint_errors.is_empty() {
                println!("  lint:");
                for err in &report.lint_errors {
                    println!("    - {err}");
                }
            }
            if !report.missing_components.is_empty() {
                println!("  missing components:");
                for comp in &report.missing_components {
                    println!("    - {comp}");
                }
            }
            if !report.errors.is_empty() {
                println!("  flow:");
                for err in &report.errors {
                    println!("    - {err}");
                }
            }
            if report.is_success() {
                println!("  result: ok");
            } else {
                println!("  result: failed");
            }
        } else {
            println!("  result: ok");
        }
    }

    fn print_conformance_report(&self, report: &ConformanceReport) {
        println!("Pack {} ({})", report.pack_id, report.pack_path.display());
        println!("  conformance:");
        for step in &report.steps {
            let status = match step.status {
                ConformanceStatus::Ok => "ok",
                ConformanceStatus::Skipped => "skipped",
                ConformanceStatus::Failed => "failed",
            };
            println!("    - {}: {}", step.name, status);
            for detail in &step.details {
                println!("      - {}", detail);
            }
        }
    }

    fn fixture_index(&self, fixture_id: &str) -> Result<usize> {
        self.fixtures
            .iter()
            .position(|f| f.id == fixture_id)
            .ok_or_else(|| anyhow!("unknown fixture {fixture_id}"))
    }

    fn show_status(
        &self,
        fixture: &Fixture,
        adapters: &[AdapterTarget],
        index: usize,
    ) -> Result<()> {
        println!("------------------------------");
        println!(
            "Fixture [{}/{}]: {}",
            index + 1,
            self.fixtures.len(),
            fixture.id
        );
        println!("Path: {}", fixture.path.display());
        for (idx, adapter) in adapters.iter().enumerate() {
            println!(
                "[{idx}] {name} ({status}){details}",
                name = adapter.name.as_str(),
                status = if adapter.enabled {
                    "enabled"
                } else {
                    "disabled"
                },
                details = adapter
                    .reason
                    .as_ref()
                    .map(|reason| format!(" - {reason}"))
                    .unwrap_or_default()
            );
        }
        println!("Commands: Enter=send, r=resend, n=next, p=prev, a=toggle, q=quit");
        Ok(())
    }

    fn toggle_adapters(&self, adapters: &mut [AdapterTarget]) -> Result<()> {
        println!("Enter indexes (comma) to toggle or 'all':");
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let trimmed = input.trim();
        if trimmed.eq_ignore_ascii_case("all") {
            for adapter in adapters {
                adapter.enabled = !adapter.enabled;
            }
            return Ok(());
        }
        for part in trimmed.split(',') {
            let idx = match part.trim().parse::<usize>() {
                Ok(i) => i,
                Err(_) => continue,
            };
            if let Some(adapter) = adapters.get_mut(idx) {
                adapter.enabled = !adapter.enabled;
            }
        }
        Ok(())
    }

    fn process_fixture(&self, fixture: &Fixture, adapters: &mut [AdapterTarget]) -> Result<()> {
        println!("Processing {}", fixture.id);
        let mut card = fixture.card.clone();
        if let Some(adaptive) = card.adaptive.as_mut() {
            flatten_column_sets(adaptive);
        }
        let spec = self.engine.render_spec(&card).context("render spec")?;
        for adapter in adapters {
            if !adapter.enabled {
                println!(" - {}: disabled", adapter.name.as_str());
                continue;
            }
            let payload = if let Some(snapshot) = self
                .engine
                .render_snapshot(adapter.platform.as_str(), &spec)
            {
                snapshot.output.payload
            } else {
                println!(
                    " - {}: render snapshot not available for platform {}",
                    adapter.name.as_str(),
                    adapter.platform.as_str()
                );
                continue;
            };
            self.persist_artifacts(fixture, adapter, &payload)?;
            println!(" - {} -> recorded", adapter.name.as_str());
        }
        Ok(())
    }

    fn persist_artifacts(
        &self,
        fixture: &Fixture,
        adapter: &AdapterTarget,
        payload: &Value,
    ) -> Result<()> {
        let base = PathBuf::from(".gsm-test")
            .join("artifacts")
            .join(&fixture.id)
            .join(adapter.name.as_str());
        fs::create_dir_all(&base).context("create artifact dir")?;
        let translated = sorted_and_redacted(payload);
        fs::write(
            base.join("translated.json"),
            serde_json::to_string_pretty(&translated)?,
        )
        .context("write translated payload")?;
        let response = json!({
            "mode": format!("{:?}", adapter.mode),
            "platform": adapter.platform.as_str(),
        });
        fs::write(
            base.join("response.json"),
            serde_json::to_string_pretty(&response)?,
        )
        .context("write response payload")?;
        Ok(())
    }

    fn run_interactive_pack(&self, fixture_id: &str, dry_run: bool) -> Result<()> {
        let mut index = self.fixture_index(fixture_id)?;
        let mode = self.adapter_mode(Some(dry_run));
        let mut adapters = self.load_pack_adapters(mode)?;
        let runtime = Self::build_runtime()?;
        let runner = self.build_pack_runner(dry_run)?;
        loop {
            let fixture = &self.fixtures[index];
            self.show_status_pack(fixture, &adapters, index)?;
            let mut input = String::new();
            io::stdin().read_line(&mut input)?;
            match input.trim() {
                "" | "r" => {
                    self.process_fixture_pack(fixture, &mut adapters, &runtime, runner.as_ref())?
                }
                "n" => index = (index + 1) % self.fixtures.len(),
                "p" => {
                    index = if index == 0 {
                        self.fixtures.len() - 1
                    } else {
                        index - 1
                    }
                }
                "a" => self.toggle_pack_adapters(&mut adapters)?,
                "q" => break,
                cmd => println!("Unknown command: {cmd}"),
            }
        }
        Ok(())
    }

    fn run_all_pack(&self, dry_run: bool) -> Result<()> {
        let mode = self.adapter_mode(Some(dry_run));
        let mut adapters = self.load_pack_adapters(mode)?;
        let runtime = Self::build_runtime()?;
        let runner = self.build_pack_runner(dry_run)?;
        for fixture in &self.fixtures {
            self.process_fixture_pack(fixture, &mut adapters, &runtime, runner.as_ref())?;
        }
        Ok(())
    }

    fn show_status_pack(
        &self,
        fixture: &Fixture,
        adapters: &[PackAdapter],
        index: usize,
    ) -> Result<()> {
        println!("------------------------------");
        println!(
            "Fixture [{}/{}]: {}",
            index + 1,
            self.fixtures.len(),
            fixture.id
        );
        println!("Path: {}", fixture.path.display());
        for (idx, adapter) in adapters.iter().enumerate() {
            println!(
                "[{idx}] {name} ({status}){details}",
                name = adapter.target.name.as_str(),
                status = if adapter.target.enabled {
                    "enabled"
                } else {
                    "disabled"
                },
                details = adapter
                    .target
                    .reason
                    .as_ref()
                    .map(|reason| format!(" - {reason}"))
                    .unwrap_or_default()
            );
        }
        println!("Commands: Enter=send, r=resend, n=next, p=prev, a=toggle, q=quit");
        Ok(())
    }

    fn toggle_pack_adapters(&self, adapters: &mut [PackAdapter]) -> Result<()> {
        println!("Enter indexes (comma) to toggle or 'all':");
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let trimmed = input.trim();
        if trimmed.eq_ignore_ascii_case("all") {
            for adapter in adapters {
                adapter.target.enabled = !adapter.target.enabled;
            }
            return Ok(());
        }
        for part in trimmed.split(',') {
            let idx = match part.trim().parse::<usize>() {
                Ok(i) => i,
                Err(_) => continue,
            };
            if let Some(adapter) = adapters.get_mut(idx) {
                adapter.target.enabled = !adapter.target.enabled;
            }
        }
        Ok(())
    }

    fn process_fixture_pack(
        &self,
        fixture: &Fixture,
        adapters: &mut [PackAdapter],
        runtime: &Runtime,
        runner: &dyn RunnerClient,
    ) -> Result<()> {
        println!("Processing {}", fixture.id);
        let chat_id = self.pack_chat_id()?;
        for adapter in adapters {
            if !adapter.target.enabled {
                println!(" - {}: disabled", adapter.target.name.as_str());
                continue;
            }
            let out = self.build_out_message(fixture, adapter.target.platform.clone(), &chat_id);
            self.persist_pack_artifacts(fixture, adapter, &out)?;
            runtime
                .block_on(runner.invoke_adapter(&out, &adapter.descriptor))
                .with_context(|| {
                    format!(
                        "runner invocation failed for {}",
                        adapter.target.name.as_str()
                    )
                })?;
            println!(" - {} -> sent", adapter.target.name.as_str());
        }
        Ok(())
    }

    fn load_pack_adapters(&self, mode: AdapterMode) -> Result<Vec<PackAdapter>> {
        let registry = self.load_pack_registry()?;
        let mut adapters = Vec::new();
        let platform_override = self
            .cli
            .platform
            .as_deref()
            .map(|value| value.parse::<Platform>())
            .transpose()
            .context("invalid --platform")?;
        for descriptor in registry.all() {
            if !descriptor.allows_egress() {
                continue;
            }
            let platform = self
                .infer_platform(&descriptor.name)
                .or(platform_override.clone())
                .ok_or_else(|| {
                    anyhow!(
                        "unable to infer platform for adapter {}; use --platform",
                        descriptor.name
                    )
                })?;
            adapters.push(PackAdapter {
                target: AdapterTarget {
                    name: descriptor.name.clone(),
                    platform,
                    enabled: true,
                    reason: None,
                    mode,
                },
                descriptor,
            });
        }
        if adapters.is_empty() {
            return Err(anyhow!(
                "no egress adapters found in packs; ensure the pack exposes adapters"
            ));
        }
        Ok(adapters)
    }

    fn load_pack_registry(&self) -> Result<AdapterRegistry> {
        if self.cli.pack_paths.is_empty() {
            return Err(anyhow!("no pack paths provided"));
        }
        AdapterRegistry::load_from_paths(&self.cli.packs_root, &self.cli.pack_paths)
            .context("load pack adapters")
    }

    fn infer_platform(&self, name: &str) -> Option<Platform> {
        infer_platform_from_adapter_name(name).or_else(|| infer_platform_from_tokens(name))
    }

    fn pack_chat_id(&self) -> Result<String> {
        self.cli
            .chat_id
            .clone()
            .ok_or_else(|| anyhow!("--chat-id is required for pack-based runs"))
    }

    fn build_out_message(
        &self,
        fixture: &Fixture,
        platform: Platform,
        chat_id: &str,
    ) -> OutMessage {
        let mut meta = BTreeMap::new();
        meta.insert("fixture_id".to_string(), Value::String(fixture.id.clone()));
        let ctx = build_tenant_ctx(&self.cli.env, &self.cli.tenant, &self.cli.team);
        OutMessage {
            ctx,
            tenant: self.cli.tenant.clone(),
            platform,
            chat_id: chat_id.to_string(),
            thread_id: None,
            kind: OutKind::Card,
            text: None,
            message_card: None,
            adaptive_card: Some(fixture.card.clone()),
            meta,
        }
    }

    fn persist_pack_artifacts(
        &self,
        fixture: &Fixture,
        adapter: &PackAdapter,
        out: &OutMessage,
    ) -> Result<()> {
        let base = PathBuf::from(".gsm-test")
            .join("artifacts")
            .join(&fixture.id)
            .join(adapter.target.name.as_str());
        fs::create_dir_all(&base).context("create artifact dir")?;
        let payload = json!({
            "adapter": {
                "name": adapter.descriptor.name.as_str(),
                "component": adapter.descriptor.component.as_str(),
                "flow": adapter.descriptor.flow_path(),
            },
            "message": out,
        });
        fs::write(
            base.join("invocation.json"),
            serde_json::to_string_pretty(&payload)?,
        )
        .context("write invocation payload")?;
        Ok(())
    }

    fn build_pack_runner(&self, dry_run: bool) -> Result<Box<dyn RunnerClient>> {
        if dry_run {
            return Ok(Box::new(LoggingRunnerClient));
        }
        let url = self
            .cli
            .runner_url
            .clone()
            .ok_or_else(|| anyhow!("--runner-url is required for pack-based runs"))?;
        let client = HttpRunnerClient::new(url, self.cli.runner_api_key.clone())?;
        Ok(Box::new(client))
    }

    fn pack_mode_enabled(&self) -> bool {
        !self.cli.pack_paths.is_empty()
    }

    fn build_runtime() -> Result<Runtime> {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("build async runtime")
    }
}

fn infer_platform_from_tokens(name: &str) -> Option<Platform> {
    let lowered = name.to_ascii_lowercase();
    for (token, platform) in [
        ("slack", Platform::Slack),
        ("teams", Platform::Teams),
        ("telegram", Platform::Telegram),
        ("whatsapp", Platform::WhatsApp),
        ("webchat", Platform::WebChat),
        ("webex", Platform::Webex),
    ] {
        if lowered.contains(token) {
            return Some(platform);
        }
    }
    None
}

fn build_tenant_ctx(env: &str, tenant: &str, team: &str) -> TenantCtx {
    let mut ctx = TenantCtx::new(EnvId(env.to_string()), TenantId(tenant.to_string()));
    ctx = ctx.with_team(Some(TeamId(team.to_string())));
    ctx
}

fn sorted_and_redacted(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries = map.iter().collect::<Vec<_>>();
            entries.sort_by_key(|(k, _)| *k);
            let mut result = serde_json::Map::new();
            for (key, val) in entries {
                if key.to_lowercase().contains("token") || key.to_lowercase().contains("secret") {
                    result.insert(key.clone(), Value::String("<redacted>".into()));
                } else {
                    result.insert(key.clone(), sorted_and_redacted(val));
                }
            }
            Value::Object(result)
        }
        Value::Array(list) => Value::Array(list.iter().map(sorted_and_redacted).collect()),
        other => other.clone(),
    }
}

fn flatten_column_sets(value: &mut Value) {
    match value {
        Value::Object(map) => {
            if let Some(body) = map.get_mut("body").and_then(Value::as_array_mut) {
                flatten_body(body);
            }
            for val in map.values_mut() {
                flatten_column_sets(val);
            }
        }
        Value::Array(arr) => {
            flatten_body(arr);
        }
        _ => {}
    }
}

fn flatten_body(body: &mut Vec<Value>) {
    let mut idx = 0;
    while idx < body.len() {
        if let Some(obj) = body[idx].as_object()
            && obj.get("type").and_then(|v| v.as_str()) == Some("ColumnSet")
        {
            let replacements = collect_column_items(obj);
            body.splice(idx..idx + 1, replacements);
            continue;
        }
        flatten_column_sets(&mut body[idx]);
        idx += 1;
    }
}

fn collect_column_items(column_set: &serde_json::Map<String, Value>) -> Vec<Value> {
    let mut result = Vec::new();
    if let Some(columns) = column_set.get("columns").and_then(Value::as_array) {
        for column in columns {
            if let Some(items) = column
                .as_object()
                .and_then(|col| col.get("items"))
                .and_then(Value::as_array)
            {
                for item in items {
                    let mut clone = item.clone();
                    flatten_column_sets(&mut clone);
                    result.push(clone);
                }
            }
        }
    }
    result
}
