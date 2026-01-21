use std::path::PathBuf;

use clap::{ArgAction, Parser, Subcommand};

use crate::adapters::AdapterConfig;

#[derive(Parser, Debug)]
#[command(name = "gsm-test", about = "MessageCard adapter validation utility")]
pub struct Cli {
    /// Path to the fixtures directory (defaults to libs/core/tests/fixtures/cards)
    #[arg(long, default_value = "libs/core/tests/fixtures/cards")]
    pub fixtures: PathBuf,

    /// Force dry-run even if adapters have tokens
    #[arg(long)]
    pub dry_run: bool,

    /// Pack files to load (repeatable); when set, run/all invoke pack adapters via runner
    #[arg(long = "pack", value_name = "FILE", global = true)]
    pub pack_paths: Vec<PathBuf>,

    /// Packs root for resolving relative pack paths
    #[arg(long = "packs-root", default_value = "packs", global = true)]
    pub packs_root: PathBuf,

    /// Runner invoke URL used for pack-based execution
    #[arg(long = "runner-url", global = true)]
    pub runner_url: Option<String>,

    /// Runner API key for pack-based execution
    #[arg(long = "runner-api-key", global = true)]
    pub runner_api_key: Option<String>,

    /// Environment identifier for pack-based messages
    #[arg(long, default_value = "dev", global = true)]
    pub env: String,

    /// Tenant identifier for pack-based messages
    #[arg(long, default_value = "ci", global = true)]
    pub tenant: String,

    /// Team identifier for pack-based messages
    #[arg(long, default_value = "ci", global = true)]
    pub team: String,

    /// Chat identifier for pack-based messages
    #[arg(long = "chat-id", global = true, allow_hyphen_values = true)]
    pub chat_id: Option<String>,

    /// Platform override for adapters with unknown platform
    #[arg(long = "platform", global = true)]
    pub platform: Option<String>,

    #[command(subcommand)]
    pub command: CliCommand,
}

#[derive(Subcommand, Debug)]
pub enum CliCommand {
    /// List fixtures and adapters
    List,
    /// Show discovered fixtures
    Fixtures,
    /// Show adapters with status
    Adapters,
    /// Run a fixture
    Run {
        /// Fixture identifier
        fixture: String,
        /// Dry-run mode
        #[arg(long)]
        dry_run: bool,
    },
    /// Run all fixtures (dry-run only in legacy renderer mode)
    All {
        /// Dry-run mode
        #[arg(long)]
        dry_run: bool,
    },
    /// Generate golden files from artifacts
    GenGolden,
    /// Work with provider gtpack bundles
    Packs {
        #[command(subcommand)]
        command: Box<PacksCommand>,
    },
    /// Run messaging provider end-to-end conformance checks
    E2e {
        /// Directory containing messaging-*.gtpack bundles
        #[arg(long, value_name = "DIR", required = true)]
        packs: PathBuf,
        /// Optional provider filter (pack id or filename substring)
        #[arg(long)]
        provider: Option<String>,
        /// Optional JSON report output path
        #[arg(long)]
        report: Option<PathBuf>,
        /// Dry-run mode (default true)
        #[arg(
            long,
            default_value_t = true,
            action = ArgAction::Set,
            num_args = 0..=1,
            default_missing_value = "true"
        )]
        dry_run: bool,
        /// Enable live network calls (requires RUN_LIVE_TESTS=true and RUN_LIVE_HTTP=true)
        #[arg(long)]
        live: bool,
        /// Enable trace logging
        #[arg(long)]
        trace: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum PacksCommand {
    /// List discovered packs and their flows
    List {
        #[command(flatten)]
        discovery: PackDiscoveryArgs,
    },
    /// Run a single pack (validates flow wiring; dry-run safe)
    Run {
        /// Path to the pack file
        pack: PathBuf,
        #[command(flatten)]
        discovery: PackDiscoveryArgs,
        #[command(flatten)]
        runtime: PackRuntimeArgs,
    },
    /// Run all discovered packs
    All {
        #[command(flatten)]
        discovery: PackDiscoveryArgs,
        #[command(flatten)]
        runtime: PackRuntimeArgs,
        /// Stop on first failure
        #[arg(long)]
        fail_fast: bool,
    },
    /// Run end-to-end conformance for pack flows
    Conformance {
        #[command(flatten)]
        discovery: PackDiscoveryArgs,
        #[command(flatten)]
        runtime: PackRuntimeArgs,
        /// Explicit pack paths to validate (repeatable)
        #[arg(long, value_name = "PATH")]
        pack: Vec<PathBuf>,
        /// Public base URL injected into setup flows
        #[arg(long, default_value = "https://example.invalid")]
        public_base_url: String,
        /// Fixture used to simulate ingress payloads
        #[arg(
            long,
            default_value = "crates/messaging-test/tests/fixtures/ingress/basic.json"
        )]
        ingress_fixture: PathBuf,
        /// Only run requirements + setup flow
        #[arg(long)]
        setup_only: bool,
    },
}

#[derive(clap::Args, Debug, Clone)]
pub struct PackDiscoveryArgs {
    /// Directory roots to scan for .gtpack files
    #[arg(long = "packs", value_name = "DIR", num_args = 0.., default_values = ["dist/packs"])]
    pub roots: Vec<PathBuf>,
    /// Glob pattern to match pack files
    #[arg(long, default_value = "messaging-*.gtpack")]
    pub glob: String,
}

#[derive(clap::Args, Debug, Clone)]
pub struct PackRuntimeArgs {
    /// Flow identifier to run (defaults to 'smoke' or the first flow)
    #[arg(long)]
    pub flow: Option<String>,
    /// Environment used for secret resolution
    #[arg(long, default_value = "dev")]
    pub env: String,
    /// Tenant used for secret resolution
    #[arg(long, default_value = "ci")]
    pub tenant: String,
    /// Team used for secret resolution
    #[arg(long, default_value = "ci")]
    pub team: String,
    /// Allow tag-only component refs (use with caution; prefer digest pins)
    #[arg(long)]
    pub allow_tags: bool,
    /// Offline mode (fail if components are not already in cache)
    #[arg(long)]
    pub offline: bool,
    /// Dry-run mode (no outbound provider calls)
    #[arg(
        long,
        default_value_t = true,
        action = ArgAction::Set,
        num_args = 0..=1,
        default_missing_value = "true"
    )]
    pub dry_run: bool,
    /// Resolve and materialize components via distributor-client (default: on)
    #[arg(
        long = "resolve-components",
        default_value_t = true,
        action = ArgAction::SetTrue
    )]
    #[arg(long = "no-resolve-components", action = ArgAction::SetFalse)]
    pub resolve_components: bool,
}

impl Cli {
    pub fn adapter_config(&self) -> AdapterConfig {
        AdapterConfig::load(self.dry_run)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packs_all_defaults_to_dry_run() {
        let cli = Cli::try_parse_from(["cli", "packs", "all"]).expect("parse cli");
        match cli.command {
            CliCommand::Packs { command } => match *command {
                PacksCommand::All { runtime, .. } => assert!(runtime.dry_run),
                other => panic!("unexpected packs command parsed: {other:?}"),
            },
            other => panic!("unexpected command parsed: {other:?}"),
        }
    }

    #[test]
    fn packs_list_accepts_multiple_roots() {
        let cli = Cli::try_parse_from([
            "cli",
            "packs",
            "list",
            "--packs",
            "dist/packs",
            "--packs",
            "extra/packs",
        ])
        .expect("parse cli");
        match cli.command {
            CliCommand::Packs { command } => match *command {
                PacksCommand::List { discovery, .. } => {
                    assert_eq!(discovery.roots.len(), 2);
                    assert!(discovery.roots.contains(&PathBuf::from("dist/packs")));
                    assert!(discovery.roots.contains(&PathBuf::from("extra/packs")));
                }
                other => panic!("unexpected packs command parsed: {other:?}"),
            },
            other => panic!("unexpected command parsed: {other:?}"),
        }
    }

    #[test]
    fn pack_flags_parse_for_run() {
        let cli = Cli::try_parse_from([
            "cli",
            "run",
            "card.basic",
            "--pack",
            "/tmp/messaging-telegram.gtpack",
            "--runner-url",
            "http://localhost:8081/invoke",
            "--chat-id",
            "-100123456",
            "--env",
            "dev",
            "--tenant",
            "acme",
            "--team",
            "default",
        ])
        .expect("parse cli");
        assert_eq!(
            cli.pack_paths,
            vec![PathBuf::from("/tmp/messaging-telegram.gtpack")]
        );
        assert_eq!(
            cli.runner_url.as_deref(),
            Some("http://localhost:8081/invoke")
        );
        assert_eq!(cli.chat_id.as_deref(), Some("-100123456"));
        assert_eq!(cli.env, "dev");
        assert_eq!(cli.tenant, "acme");
        assert_eq!(cli.team, "default");
    }
}
