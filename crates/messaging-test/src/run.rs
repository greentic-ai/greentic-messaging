use std::fs;
use std::io;
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};

use crate::adapters::{AdapterConfig, AdapterMode, AdapterTarget, registry_from_env};
use crate::cli::{Cli, CliCommand};
use crate::fixtures::{Fixture, discover};
use gsm_core::{TenantCtx, make_tenant_ctx};

pub struct RunContext {
    cli: Cli,
    fixtures: Vec<Fixture>,
    config: AdapterConfig,
    ctx: TenantCtx,
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
        let ctx = make_tenant_ctx("local-dev".into(), None, None);
        Ok(Self {
            cli,
            fixtures,
            config,
            ctx,
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
                    Some(reason) => format!(" ({})", reason),
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

    fn fixture_index(&self, fixture_id: &str) -> Result<usize> {
        self.fixtures
            .iter()
            .position(|f| f.id == fixture_id)
            .ok_or_else(|| anyhow!("unknown fixture {}", fixture_id))
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
                name = adapter.name,
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
        for adapter in adapters {
            if !adapter.enabled {
                println!(" - {}: disabled", adapter.name);
                continue;
            }
            let payload = adapter.sender.translate(&self.ctx, &fixture.card)?;
            let result = adapter.sender.send(&self.ctx, &payload)?;
            self.persist_artifacts(fixture, adapter, &payload.0, &result)?;
            println!(" - {} -> ok={}", adapter.name, result.ok);
        }
        Ok(())
    }

    fn persist_artifacts(
        &self,
        fixture: &Fixture,
        adapter: &AdapterTarget,
        payload: &Value,
        result: &crate::adapters::SendResult,
    ) -> Result<()> {
        let base = PathBuf::from(".gsm-test")
            .join("artifacts")
            .join(&fixture.id)
            .join(adapter.name);
        fs::create_dir_all(&base).context("create artifact dir")?;
        let translated = sorted_and_redacted(payload);
        fs::write(
            base.join("translated.json"),
            serde_json::to_string_pretty(&translated)?,
        )
        .context("write translated payload")?;
        let response = json!({
            "ok": result.ok,
            "message_id": result.message_id,
            "diagnostics": result.diagnostics,
            "mode": format!("{:?}", adapter.mode),
        });
        fs::write(
            base.join("response.json"),
            serde_json::to_string_pretty(&response)?,
        )
        .context("write response payload")?;
        Ok(())
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::fs;
    use std::io::Write;
    use std::path::Path;
    use tempfile::tempdir;

    fn sample_cli(fixtures_dir: PathBuf) -> Cli {
        Cli {
            fixtures: fixtures_dir,
            dry_run: true,
            command: CliCommand::All { dry_run: true },
        }
    }

    fn prepare_fixture(dir: &Path) -> PathBuf {
        let path = dir.join("test-card.json");
        let mut file = fs::File::create(&path).unwrap();
        writeln!(
            file,
            "{}",
            json!({
                "title": "Hello",
                "text": "World",
                "images": [],
                "actions": []
            })
        )
        .unwrap();
        path
    }

    fn set_adapter_env() {
        let keys = [
            "MS_GRAPH_TOKEN",
            "WEBEX_BOT_TOKEN",
            "SLACK_BOT_TOKEN",
            "WEBCHAT_SECRET",
            "TELEGRAM_BOT_TOKEN",
            "WHATSAPP_TOKEN",
        ];
        for key in keys {
            unsafe {
                env::set_var(key, "test");
            }
        }
    }

    #[test]
    fn run_all_generates_artifacts() {
        let fixtures_dir = tempdir().unwrap();
        prepare_fixture(fixtures_dir.path());
        set_adapter_env();
        let cli = sample_cli(fixtures_dir.path().to_path_buf());
        let ctx = RunContext::new(cli).expect("context");
        ctx.run_all(true).expect("run all");
        let artifacts = PathBuf::from(".gsm-test/artifacts");
        assert!(artifacts.exists());
        fs::remove_dir_all(".gsm-test").ok();
    }
}
