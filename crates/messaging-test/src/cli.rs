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
    /// Run all fixtures (dry-run only)
    All {
        /// Dry-run mode (required)
        #[arg(long)]
        dry_run: bool,
    },
    /// Generate golden files from artifacts
    GenGolden,
    /// Work with provider gtpack bundles
    Packs {
        #[command(subcommand)]
        command: PacksCommand,
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
    /// Dry-run mode (no outbound provider calls)
    #[arg(
        long,
        default_value_t = true,
        action = ArgAction::Set,
        num_args = 0..=1,
        default_missing_value = "true"
    )]
    pub dry_run: bool,
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
            CliCommand::Packs {
                command: PacksCommand::All { runtime, .. },
            } => assert!(runtime.dry_run),
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
            CliCommand::Packs {
                command: PacksCommand::List { discovery, .. },
            } => {
                assert_eq!(discovery.roots.len(), 2);
                assert!(discovery.roots.contains(&PathBuf::from("dist/packs")));
                assert!(discovery.roots.contains(&PathBuf::from("extra/packs")));
            }
            other => panic!("unexpected command parsed: {other:?}"),
        }
    }
}
