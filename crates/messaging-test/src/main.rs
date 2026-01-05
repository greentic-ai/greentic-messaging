mod adapters;
mod cli;
mod fixtures;
mod packs;
mod run;

use anyhow::Result;
use clap::Parser;

use crate::cli::Cli;
use crate::run::RunContext;

fn main() -> Result<()> {
    let cli = Cli::parse();
    let ctx = RunContext::new(cli)?;
    ctx.execute()
}
