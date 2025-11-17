mod platform;
mod uri;

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use greentic_secrets::core::embedded::SecretsCore;
use serde_json::Value;
use tokio::io::{self, AsyncRead, AsyncReadExt};

use crate::platform::PlatformArg;
use crate::uri::{build_credentials_uri, build_placeholder_uri};

#[derive(Parser)]
#[command(
    name = "messaging-tenants",
    version,
    about = "Bootstrap tenants in greentic-secrets"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Ensure an environment can be referenced in the secrets backend.
    InitEnv {
        /// Environment identifier (e.g. dev, test, prod).
        #[arg(long)]
        env: String,
    },
    /// Create placeholder secrets for a tenant/team combination.
    AddTenant {
        /// Environment identifier.
        #[arg(long)]
        env: String,
        /// Tenant identifier.
        #[arg(long)]
        tenant: String,
        /// Team identifier. Repeatable; default is 'default'.
        #[arg(long)]
        team: Vec<String>,
    },
    /// Write credentials JSON for a platform.
    SetCredentials {
        /// Environment identifier.
        #[arg(long)]
        env: String,
        /// Tenant identifier.
        #[arg(long)]
        tenant: String,
        /// Team identifier.
        #[arg(long)]
        team: Option<String>,
        /// Platform (slack, teams, telegram, whatsapp, webchat, webex).
        #[arg(value_enum)]
        platform: PlatformArg,
        /// File containing the credentials JSON. Defaults to STDIN when omitted.
        #[arg(short, long, value_name = "FILE")]
        file: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::InitEnv { env } => init_env(env).await,
        Command::AddTenant { env, tenant, team } => add_tenant(env, tenant, team).await,
        Command::SetCredentials {
            env,
            tenant,
            team,
            platform,
            file,
        } => set_credentials(env, tenant, team, platform, file).await,
    }
}

async fn init_env(env: String) -> Result<()> {
    let env = normalize(&env);
    let core = build_core("default").await?;
    let prefix = format!("secret://{env}/default/_/messaging");
    core.list(&prefix)
        .await
        .context("failed to probe secrets backend for environment")?;
    println!("Environment '{env}' is reachable in the secrets backend.");
    Ok(())
}

async fn add_tenant(env: String, tenant: String, teams: Vec<String>) -> Result<()> {
    let env = normalize(&env);
    let tenant = normalize(&tenant);
    let teams = if teams.is_empty() {
        vec!["default".into()]
    } else {
        teams
    };

    let core = build_core(&tenant).await?;

    for raw_team in teams {
        let team = normalize(&raw_team);
        let uri = build_placeholder_uri(&env, &tenant, &team)?;
        core.put_json(&uri, &Value::Object(Default::default()))
            .await
            .with_context(|| format!("failed to create placeholder for team '{team}'"))?;
        println!("Created placeholder secret for team '{team}'.");
    }
    Ok(())
}

async fn set_credentials(
    env: String,
    tenant: String,
    team: Option<String>,
    platform: PlatformArg,
    file: Option<PathBuf>,
) -> Result<()> {
    let env = normalize(&env);
    let tenant = normalize(&tenant);
    let team_name = team
        .as_deref()
        .map(normalize)
        .unwrap_or_else(|| "default".into());

    let mut source: Box<dyn AsyncRead + Unpin + Send> = if let Some(path) = file {
        Box::new(
            tokio::fs::File::open(path)
                .await
                .context("failed to open credentials file")?,
        )
    } else {
        Box::new(io::stdin())
    };
    let mut buffer = Vec::new();
    source
        .read_to_end(&mut buffer)
        .await
        .context("failed to read credentials payload")?;
    let value: Value = serde_json::from_slice(&buffer).context("failed to parse JSON payload")?;

    let uri = build_credentials_uri(&env, &tenant, Some(team_name.as_str()), platform.into())?;
    let core = build_core(&tenant).await?;
    core.put_json(&uri, &value)
        .await
        .context("failed to write credentials secret")?;
    println!(
        "Stored credentials for platform '{}' at '{}'.",
        platform.as_str(),
        uri
    );
    Ok(())
}

async fn build_core(tenant: &str) -> Result<SecretsCore> {
    SecretsCore::builder()
        .tenant(tenant.to_string())
        .build()
        .await
        .map_err(Into::into)
}

fn normalize(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}
