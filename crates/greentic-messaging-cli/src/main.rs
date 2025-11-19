use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, Stdio},
};

use anyhow::{Context, Result, anyhow};
use clap::{Parser, Subcommand, ValueEnum};

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        CliCommand::Info => handle_info(),
        CliCommand::Dev { command } => handle_dev(command),
        CliCommand::Serve {
            kind,
            platform,
            tenant,
            team,
        } => handle_serve(kind, platform, tenant, team),
        CliCommand::Flows { command } => handle_flows(command),
        CliCommand::Test { command } => handle_test(command),
        CliCommand::Admin { command } => handle_admin(command),
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "greentic-messaging",
    version,
    about = "Unified CLI for Greentic messaging services"
)]
struct Cli {
    #[command(subcommand)]
    command: CliCommand,
}

#[derive(Subcommand, Debug)]
enum CliCommand {
    /// Inspect the current environment, secrets and available binaries
    Info,
    /// Developer utilities (coming soon)
    Dev {
        #[command(subcommand)]
        command: DevCommand,
    },
    /// Serve ingress/egress/subscriptions
    Serve {
        #[arg(value_enum)]
        kind: ServeKind,
        platform: String,
        #[arg(long)]
        tenant: String,
        #[arg(long)]
        team: Option<String>,
    },
    /// Flow helpers (coming soon)
    Flows {
        #[command(subcommand)]
        command: FlowCommand,
    },
    /// Test wrappers for greentic-messaging-test
    Test {
        #[command(subcommand)]
        command: TestCommand,
    },
    /// Admin helpers (guard rails, Slack/Teams setup)
    Admin {
        #[command(subcommand)]
        command: AdminCommand,
    },
}

#[derive(Subcommand, Debug)]
enum DevCommand {
    Up,
}

#[derive(Subcommand, Debug)]
enum FlowCommand {
    Run {
        #[arg(long)]
        flow: PathBuf,
        #[arg(long)]
        platform: String,
        #[arg(long)]
        tenant: String,
        #[arg(long)]
        team: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
enum TestCommand {
    Fixtures,
    Adapters,
    Run {
        fixture: String,
        #[arg(long)]
        dry_run: bool,
    },
    All {
        #[arg(long)]
        dry_run: bool,
    },
    GenGolden,
}

#[derive(Subcommand, Debug)]
enum AdminCommand {
    #[command(name = "guard-rails")]
    GuardRails {
        #[command(subcommand)]
        command: GuardRailsCommand,
    },
    Slack {
        #[command(subcommand)]
        command: SlackAdminCommand,
    },
    Teams {
        #[command(subcommand)]
        command: TeamsAdminCommand,
    },
    Telegram {
        #[command(subcommand)]
        command: TelegramAdminCommand,
    },
    #[command(name = "whatsapp")]
    WhatsApp {
        #[command(subcommand)]
        command: WhatsAppAdminCommand,
    },
}

#[derive(Subcommand, Debug)]
enum GuardRailsCommand {
    Show,
    #[command(name = "sample-env")]
    SampleEnv,
}

#[derive(Subcommand, Debug)]
enum SlackAdminCommand {
    #[command(name = "oauth-helper")]
    OauthHelper {
        /// Extra arguments passed to `cargo run -p gsm-slack-oauth -- ...`
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
}

#[derive(Subcommand, Debug)]
enum TeamsAdminCommand {
    Setup {
        /// Arguments forwarded to `cargo run --manifest-path scripts/Cargo.toml --bin teams_setup -- ...`
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
}

#[derive(Subcommand, Debug)]
enum TelegramAdminCommand {
    Setup {
        /// Arguments forwarded to `cargo run --manifest-path scripts/Cargo.toml --bin telegram_setup -- ...`
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
}

#[derive(Subcommand, Debug)]
enum WhatsAppAdminCommand {
    Setup {
        /// Arguments forwarded to `cargo run --manifest-path scripts/Cargo.toml --bin whatsapp_setup -- ...`
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum ServeKind {
    Ingress,
    Egress,
    Subscriptions,
}

#[derive(Default, Debug)]
struct TenantInfo {
    tenant: String,
    teams: Vec<String>,
}

fn handle_info() -> Result<()> {
    let env = current_env();
    println!("Environment : {env}");

    match secrets_root()? {
        Some(root) => {
            println!("Secrets dir  : {}", root.display());
            let tenants = discover_tenants(&root, &env)?;
            if tenants.is_empty() {
                println!("Tenants      : (none detected for {env})");
            } else {
                println!("Tenants      :");
                for t in tenants {
                    if t.teams.is_empty() {
                        println!("  - {} (no teams found)", t.tenant);
                    } else {
                        let teams = t.teams.join(", ");
                        println!("  - {} [teams: {teams}]", t.tenant);
                    }
                }
            }
        }
        None => println!(
            "Secrets dir  : not configured (set GREENTIC_SECRETS_DIR or provide ./secrets)"
        ),
    }

    println!("\nAvailable services:");
    println!("  ingress       : {}", INGRESS_PLATFORMS.join(", "));
    println!("  egress        : {}", EGRESS_PLATFORMS.join(", "));
    println!("  subscriptions : {}", SUBSCRIPTION_PLATFORMS.join(", "));
    println!("  runner/flows  : gsm-runner (FLOW=... PLATFORM=...)\n");

    println!("For more commands, run `greentic-messaging --help`.");
    Ok(())
}

fn handle_dev(command: DevCommand) -> Result<()> {
    match command {
        DevCommand::Up => {
            println!("Running make stack-up (best effort)…");
            if let Err(err) = run_make("stack-up", &[]) {
                eprintln!("stack-up failed: {err:?}");
            }
        }
    }
    Ok(())
}

fn handle_serve(
    kind: ServeKind,
    platform: String,
    tenant: String,
    team: Option<String>,
) -> Result<()> {
    let env_scope = current_env();
    let team = team.unwrap_or_else(|| "default".into());
    let target = serve_target(kind, &platform)?;
    let nats_url = env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
    println!(
        "Starting {kind:?} {platform} (env={env_scope}, tenant={tenant}, team={team}, nats={nats_url})"
    );
    let envs = vec![
        ("GREENTIC_ENV", env_scope.as_str()),
        ("TENANT", tenant.as_str()),
        ("TEAM", team.as_str()),
        ("NATS_URL", nats_url.as_str()),
    ];
    run_make_with_logging(&target, &envs)
}

fn handle_flows(command: FlowCommand) -> Result<()> {
    match command {
        FlowCommand::Run {
            flow,
            platform,
            tenant,
            team,
        } => {
            let env_scope = current_env();
            let flow_path = flow.canonicalize().unwrap_or(flow);
            let team = team.unwrap_or_else(|| "default".into());
            println!(
                "Running flow {} (platform={platform}, env={env_scope}, tenant={tenant}, team={team})",
                flow_path.display()
            );
            run_make_with_logging(
                "run-runner",
                &[
                    ("GREENTIC_ENV", env_scope.as_str()),
                    ("FLOW", flow_path.to_string_lossy().as_ref()),
                    ("PLATFORM", platform.as_str()),
                    ("TENANT", tenant.as_str()),
                    ("TEAM", team.as_str()),
                ],
            )
        }
    }
}

fn handle_test(command: TestCommand) -> Result<()> {
    match command {
        TestCommand::Fixtures => run_messaging_test_cli(&["fixtures"]),
        TestCommand::Adapters => run_messaging_test_cli(&["adapters"]),
        TestCommand::Run { fixture, dry_run } => {
            let mut args = vec!["run".to_string(), fixture];
            if dry_run {
                args.push("--dry-run".into());
            }
            run_messaging_test_cli_str(args)
        }
        TestCommand::All { dry_run } => {
            if !dry_run {
                return Err(anyhow!(
                    "`greentic-messaging test all` requires --dry-run for safety"
                ));
            }
            run_messaging_test_cli(&["all", "--dry-run"])
        }
        TestCommand::GenGolden => run_messaging_test_cli(&["gen-golden"]),
    }
}

fn handle_admin(command: AdminCommand) -> Result<()> {
    match command {
        AdminCommand::GuardRails { command } => match command {
            GuardRailsCommand::Show => guard_rails_show(),
            GuardRailsCommand::SampleEnv => guard_rails_sample_env(),
        },
        AdminCommand::Slack { command } => match command {
            SlackAdminCommand::OauthHelper { args } => run_slack_oauth_helper(args),
        },
        AdminCommand::Teams { command } => match command {
            TeamsAdminCommand::Setup { args } => run_teams_setup(args),
        },
        AdminCommand::Telegram { command } => match command {
            TelegramAdminCommand::Setup { args } => run_telegram_setup(args),
        },
        AdminCommand::WhatsApp { command } => match command {
            WhatsAppAdminCommand::Setup { args } => run_whatsapp_setup(args),
        },
    }
}

fn secrets_root() -> Result<Option<PathBuf>> {
    if let Ok(path) = env::var("GREENTIC_SECRETS_DIR").or_else(|_| env::var("SECRETS_ROOT")) {
        let path = PathBuf::from(path);
        if path.exists() {
            return Ok(Some(path));
        }
    }

    let cwd = env::current_dir().context("failed to resolve current directory")?;
    let candidate = cwd.join("secrets");
    if candidate.exists() {
        return Ok(Some(candidate));
    }

    Ok(None)
}

fn discover_tenants(root: &Path, env: &str) -> Result<Vec<TenantInfo>> {
    let env_dir = root.join(env);
    if !env_dir.exists() {
        return Ok(Vec::new());
    }

    let mut tenants: BTreeMap<String, TenantInfo> = BTreeMap::new();
    for tenant_entry in fs::read_dir(&env_dir)
        .with_context(|| format!("failed to enumerate tenants under {}", env_dir.display()))?
    {
        let tenant_entry = tenant_entry?;
        if !tenant_entry.file_type()?.is_dir() {
            continue;
        }
        let tenant_name = tenant_entry
            .file_name()
            .into_string()
            .unwrap_or_else(|_| "unknown".into());
        let tenant_dir = tenant_entry.path();
        let teams = collect_teams(&tenant_dir)?;
        tenants.insert(
            tenant_name.clone(),
            TenantInfo {
                tenant: tenant_name,
                teams,
            },
        );
    }

    Ok(tenants.into_values().collect())
}

fn collect_teams(tenant_dir: &Path) -> Result<Vec<String>> {
    let mut teams = Vec::new();
    for team_entry in fs::read_dir(tenant_dir)
        .with_context(|| format!("failed to read {}", tenant_dir.display()))?
    {
        let team_entry = team_entry?;
        if !team_entry.file_type()?.is_dir() {
            continue;
        }
        let name = team_entry
            .file_name()
            .into_string()
            .unwrap_or_else(|_| "unknown".into());
        teams.push(name);
    }
    teams.sort();
    Ok(teams)
}

const INGRESS_PLATFORMS: &[&str] = &["slack", "telegram", "webchat", "whatsapp", "teams"];
const EGRESS_PLATFORMS: &[&str] = &["slack", "telegram", "webchat", "whatsapp", "teams", "webex"];
const SUBSCRIPTION_PLATFORMS: &[&str] = &["teams"];
const CLI_DRY_RUN_ENV: &str = "GREENTIC_MESSAGING_CLI_DRY_RUN";

fn current_env() -> String {
    env::var("GREENTIC_ENV").unwrap_or_else(|_| "dev".into())
}

fn run_make_with_logging(target: &str, envs: &[(&str, &str)]) -> Result<()> {
    println!("\n> make {target}");
    run_make(target, envs)
}

fn run_make(target: &str, envs: &[(&str, &str)]) -> Result<()> {
    if cli_dry_run() {
        println!("(dry-run) make {target}");
        return Ok(());
    }
    let mut cmd = ProcessCommand::new("make");
    cmd.arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    for (key, value) in envs {
        cmd.env(key, value);
    }
    let status = cmd
        .status()
        .with_context(|| format!("failed to run make {target}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("make {target} exited with status {status}"))
    }
}

fn serve_target(kind: ServeKind, platform: &str) -> Result<String> {
    let normalized = platform.to_ascii_lowercase();
    let list = match kind {
        ServeKind::Ingress => INGRESS_PLATFORMS,
        ServeKind::Egress => EGRESS_PLATFORMS,
        ServeKind::Subscriptions => SUBSCRIPTION_PLATFORMS,
    };
    if !list.contains(&normalized.as_str()) {
        return Err(anyhow!(
            "platform '{platform}' is not supported for {kind:?}"
        ));
    }
    let prefix = match kind {
        ServeKind::Ingress => "run-ingress-",
        ServeKind::Egress => "run-egress-",
        ServeKind::Subscriptions => "run-subscriptions-",
    };
    Ok(format!("{prefix}{normalized}"))
}

fn run_messaging_test_cli(args: &[&str]) -> Result<()> {
    run_messaging_test_cli_str(args.iter().map(|s| s.to_string()).collect())
}

fn run_messaging_test_cli_str(args: Vec<String>) -> Result<()> {
    if cli_dry_run() {
        println!(
            "(dry-run) cargo run -p greentic-messaging-test{}",
            dry_suffix(&args)
        );
        return Ok(());
    }
    let mut cmd = ProcessCommand::new("cargo");
    cmd.arg("run")
        .arg("-p")
        .arg("greentic-messaging-test")
        .arg("--")
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    for arg in args {
        cmd.arg(arg);
    }
    let status = cmd
        .status()
        .context("failed to run greentic-messaging-test")?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!(
            "greentic-messaging-test exited with status {status}"
        ))
    }
}

fn guard_rails_show() -> Result<()> {
    println!("Ingress guard rails:");
    let bearer = env::var("INGRESS_BEARER").ok();
    let bearer_detail = if bearer.is_some() {
        "Authorization header must include the configured bearer token".to_string()
    } else {
        "unset (export INGRESS_BEARER to enforce Authorization: Bearer <token>)".to_string()
    };
    print_guard_line("Bearer auth", bearer.is_some(), &bearer_detail);

    let hmac_secret = env::var("INGRESS_HMAC_SECRET").ok();
    let header = env::var("INGRESS_HMAC_HEADER").unwrap_or_else(|_| "x-signature".into());
    let hmac_detail = if hmac_secret.is_some() {
        format!("enabled (expects base64(HMAC_SHA256(body)) in {header})")
    } else {
        format!(
            "unset (export INGRESS_HMAC_SECRET and optional INGRESS_HMAC_HEADER, default {header})"
        )
    };
    print_guard_line("HMAC signature", hmac_secret.is_some(), &hmac_detail);

    let jwt_secret = env::var("JWT_SECRET").ok();
    let action_base = env::var("ACTION_BASE_URL")
        .ok()
        .filter(|s| !s.trim().is_empty());
    let jwt_alg = env::var("JWT_ALG").unwrap_or_else(|_| "HS256".into());
    let action_enabled = jwt_secret.is_some() && action_base.is_some();
    let action_detail = if action_enabled {
        format!(
            "signed action links enabled (alg={jwt_alg}, base={})",
            action_base.unwrap()
        )
    } else {
        "unset (export JWT_SECRET, ACTION_BASE_URL, and optional JWT_ALG to enable signed action links)".to_string()
    };
    print_guard_line("Action links", action_enabled, &action_detail);

    println!("\nSee README.md#admin--security-helpers for full details.");
    Ok(())
}

fn guard_rails_sample_env() -> Result<()> {
    println!("# Guard rail sample env");
    println!("# Uncomment the blocks you need and replace placeholder values.");
    println!("# Require Authorization: Bearer header on ingress:");
    println!("#INGRESS_BEARER=replace-with-shared-token");
    println!("# HMAC validation for webhook/admin calls:");
    println!("#INGRESS_HMAC_SECRET=replace-with-strong-secret");
    println!("#INGRESS_HMAC_HEADER=x-signature");
    println!("# Signed action links for MessageCard buttons:");
    println!("#JWT_SECRET=change-me");
    println!("#JWT_ALG=HS256");
    println!("#ACTION_BASE_URL=https://actions.example.dev/a");
    Ok(())
}

fn print_guard_line(name: &str, enabled: bool, detail: &str) {
    let status = if enabled { "ENABLED" } else { "disabled" };
    println!("- {name:<16} {status:>8} – {detail}");
}

fn run_slack_oauth_helper(args: Vec<String>) -> Result<()> {
    println!("Launching Slack OAuth helper (gsm-slack-oauth)...");
    run_cargo_package("greentic-messaging-cli", "gsm-slack-oauth", &args)
}

fn run_teams_setup(args: Vec<String>) -> Result<()> {
    println!("Running Teams setup helper...");
    run_cargo_manifest_bin("scripts/Cargo.toml", "teams_setup", &args)
}

fn run_telegram_setup(args: Vec<String>) -> Result<()> {
    println!("Running Telegram setup helper...");
    run_cargo_manifest_bin("scripts/Cargo.toml", "telegram_setup", &args)
}

fn run_whatsapp_setup(args: Vec<String>) -> Result<()> {
    println!("Running WhatsApp setup helper...");
    run_cargo_manifest_bin("scripts/Cargo.toml", "whatsapp_setup", &args)
}

fn run_cargo_package(invoker: &str, package: &str, args: &[String]) -> Result<()> {
    if cli_dry_run() {
        println!("(dry-run) cargo run -p {package}{}", dry_suffix(args));
        return Ok(());
    }
    let mut cmd = ProcessCommand::new("cargo");
    cmd.arg("run")
        .arg("-p")
        .arg(package)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    if !args.is_empty() {
        cmd.arg("--");
        for arg in args {
            cmd.arg(arg);
        }
    }
    let status = cmd
        .status()
        .with_context(|| format!("failed to run cargo package {package} (invoked by {invoker})"))?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("{package} exited with status {status}"))
    }
}

fn run_cargo_manifest_bin(manifest: &str, bin: &str, args: &[String]) -> Result<()> {
    if cli_dry_run() {
        println!(
            "(dry-run) cargo run --manifest-path {manifest} --bin {bin}{}",
            dry_suffix(args)
        );
        return Ok(());
    }
    let mut cmd = ProcessCommand::new("cargo");
    cmd.arg("run")
        .arg("--manifest-path")
        .arg(manifest)
        .arg("--bin")
        .arg(bin)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    if !args.is_empty() {
        cmd.arg("--");
        for arg in args {
            cmd.arg(arg);
        }
    }
    let status = cmd
        .status()
        .with_context(|| format!("failed to run cargo manifest {manifest} (bin {bin})"))?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("{bin} exited with status {status}"))
    }
}

fn cli_dry_run() -> bool {
    env::var(CLI_DRY_RUN_ENV)
        .map(|value| parse_truthy(&value))
        .unwrap_or(false)
}

fn parse_truthy(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn dry_suffix(args: &[String]) -> String {
    if args.is_empty() {
        String::new()
    } else {
        format!(" -- {}", args.join(" "))
    }
}
