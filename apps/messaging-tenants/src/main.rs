use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use tokio::process::Command;

#[derive(Parser)]
#[command(
    name = "messaging-tenants",
    version,
    about = "Convenience wrapper around the greentic-secrets CLI for messaging packs"
)]
struct Cli {
    /// Path to the greentic-secrets binary (default: looks up 'greentic-secrets' in PATH).
    #[arg(long, env = "GREENTIC_SECRETS_CLI", default_value = "greentic-secrets")]
    secrets_cli: String,

    #[command(subcommand)]
    command: CommandKind,
}

#[derive(Subcommand)]
enum CommandKind {
    /// Run greentic-secrets init for a messaging pack.
    Init {
        #[arg(long)]
        pack: PathBuf,
        #[arg(long)]
        env: Option<String>,
        #[arg(long)]
        tenant: Option<String>,
        #[arg(long)]
        team: Option<String>,
        #[arg(long)]
        non_interactive: bool,
        #[arg(long)]
        from_dotenv: Option<PathBuf>,
    },
    /// Scaffold a seed file from a pack.
    Scaffold {
        #[arg(long)]
        pack: PathBuf,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        env: Option<String>,
        #[arg(long)]
        tenant: Option<String>,
        #[arg(long)]
        team: Option<String>,
    },
    /// Fill a seed file interactively or from dotenv.
    Wizard {
        #[arg(short = 'i', long)]
        input: PathBuf,
        #[arg(short = 'o', long)]
        output: PathBuf,
        #[arg(long)]
        from_dotenv: Option<PathBuf>,
    },
    /// Apply a filled seed file to the configured backend.
    Apply {
        #[arg(short = 'f', long)]
        file: PathBuf,
        #[arg(long)]
        broker: Option<String>,
    },
    /// greentic-secrets ctx set
    CtxSet {
        #[arg(long)]
        env: String,
        #[arg(long)]
        tenant: String,
        #[arg(long)]
        team: Option<String>,
    },
    /// greentic-secrets ctx show
    CtxShow,
    /// greentic-secrets dev up
    DevUp,
    /// greentic-secrets dev down
    DevDown {
        #[arg(long)]
        destroy: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        CommandKind::Init {
            pack,
            env,
            tenant,
            team,
            non_interactive,
            from_dotenv,
        } => {
            let mut args = vec![
                "init".to_string(),
                "--pack".to_string(),
                pack.display().to_string(),
            ];
            push_opt(&mut args, "--env", env.as_deref());
            push_opt(&mut args, "--tenant", tenant.as_deref());
            push_opt(&mut args, "--team", team.as_deref());
            if non_interactive {
                args.push("--non-interactive".to_string());
            }
            if let Some(path) = from_dotenv {
                args.push("--from-dotenv".to_string());
                args.push(path.display().to_string());
            }
            run_secrets(&cli.secrets_cli, &args).await
        }
        CommandKind::Scaffold {
            pack,
            out,
            env,
            tenant,
            team,
        } => {
            let mut args = vec![
                "scaffold".to_string(),
                "--pack".to_string(),
                pack.display().to_string(),
                "--out".to_string(),
                out.display().to_string(),
            ];
            push_opt(&mut args, "--env", env.as_deref());
            push_opt(&mut args, "--tenant", tenant.as_deref());
            push_opt(&mut args, "--team", team.as_deref());
            run_secrets(&cli.secrets_cli, &args).await
        }
        CommandKind::Wizard {
            input,
            output,
            from_dotenv,
        } => {
            let mut args = vec![
                "wizard".to_string(),
                "--input".to_string(),
                input.display().to_string(),
                "--output".to_string(),
                output.display().to_string(),
            ];
            if let Some(path) = from_dotenv {
                args.push("--from-dotenv".to_string());
                args.push(path.display().to_string());
            }
            run_secrets(&cli.secrets_cli, &args).await
        }
        CommandKind::Apply { file, broker } => {
            let mut args = vec![
                "apply".to_string(),
                "--file".to_string(),
                file.display().to_string(),
            ];
            if let Some(url) = broker {
                args.push("--broker".to_string());
                args.push(url);
            }
            run_secrets(&cli.secrets_cli, &args).await
        }
        CommandKind::CtxSet { env, tenant, team } => {
            let mut args = vec![
                "ctx".to_string(),
                "set".to_string(),
                "--env".to_string(),
                env,
                "--tenant".to_string(),
                tenant,
            ];
            if let Some(team) = team {
                args.push("--team".to_string());
                args.push(team);
            }
            run_secrets(&cli.secrets_cli, &args).await
        }
        CommandKind::CtxShow => run_secrets(&cli.secrets_cli, &["ctx", "show"]).await,
        CommandKind::DevUp => run_secrets(&cli.secrets_cli, &["dev", "up"]).await,
        CommandKind::DevDown { destroy } => {
            let mut args = vec!["dev".to_string(), "down".to_string()];
            if destroy {
                args.push("--destroy".to_string());
            }
            run_secrets(&cli.secrets_cli, &args).await
        }
    }
}

async fn run_secrets<S, I, A>(binary: S, args: I) -> Result<()>
where
    S: AsRef<str>,
    I: IntoIterator<Item = A>,
    A: AsRef<str>,
{
    let binary = binary.as_ref();
    let display_args: Vec<String> = args.into_iter().map(|a| a.as_ref().to_string()).collect();
    let status = Command::new(binary)
        .args(&display_args)
        .status()
        .await
        .with_context(|| format!("failed to spawn {binary}"))?;
    if !status.success() {
        bail!("{binary} exited with status {status}");
    }
    Ok(())
}

fn push_opt(args: &mut Vec<String>, flag: &str, value: Option<&str>) {
    if let Some(value) = value {
        args.push(flag.to_string());
        args.push(value.to_string());
    }
}
