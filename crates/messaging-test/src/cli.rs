use std::path::PathBuf;

use clap::{Parser, Subcommand};

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
}

impl Cli {
    pub fn adapter_config(&self) -> AdapterConfig {
        AdapterConfig::load(self.dry_run)
    }
}
