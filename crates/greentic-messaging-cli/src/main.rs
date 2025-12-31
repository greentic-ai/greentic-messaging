use std::{
    env,
    path::PathBuf,
    process::{Command as ProcessCommand, Stdio},
};

use anyhow::{Context, Result, anyhow};
use clap::{Parser, Subcommand, ValueEnum};
use gsm_core::{
    AdapterRegistry, DefaultAdapterPacksConfig, MessagingAdapterKind, adapter_pack_paths_from_env,
    default_adapter_pack_paths,
};

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        CliCommand::Info {
            pack,
            packs_root,
            no_default_packs,
        } => handle_info(pack, packs_root, no_default_packs),
        CliCommand::Dev { command } => handle_dev(command),
        CliCommand::Serve {
            kind,
            platform,
            tenant,
            team,
            pack,
            packs_root,
            no_default_packs,
            adapter,
        } => handle_serve(
            kind,
            platform,
            tenant,
            team,
            pack,
            packs_root,
            no_default_packs,
            adapter,
        ),
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
    Info {
        /// Path to a messaging pack (.yaml or .gtpack); can be repeated.
        #[arg(long, value_name = "PATH")]
        pack: Vec<PathBuf>,
        /// Root directory that contains the packs folder (defaults to ./packs).
        #[arg(long, value_name = "PATH")]
        packs_root: Option<PathBuf>,
        /// Disable loading default packs shipped in packs/messaging.
        #[arg(long)]
        no_default_packs: bool,
    },
    /// Developer utilities (local stack helpers)
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
        /// Path to a messaging pack (.yaml or .gtpack); can be repeated.
        #[arg(long, value_name = "PATH")]
        pack: Vec<PathBuf>,
        /// Root directory that contains the packs folder (defaults to ./packs).
        #[arg(long, value_name = "PATH")]
        packs_root: Option<PathBuf>,
        /// Disable loading default packs shipped in packs/messaging.
        #[arg(long)]
        no_default_packs: bool,
        /// Override the egress adapter name (egress only).
        #[arg(long)]
        adapter: Option<String>,
    },
    /// Flow helpers
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
    /// Start the local docker/NATS stack (stack-up)
    Up,
    /// Stop and clean the local docker stack (stack-down)
    Down,
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

fn handle_info(
    pack: Vec<PathBuf>,
    packs_root: Option<PathBuf>,
    no_default_packs: bool,
) -> Result<()> {
    let env = current_env();
    println!("Environment : {env}");

    match secrets_ctx()? {
        Some(ctx) => println!("Secrets ctx  : {ctx}"),
        None => println!(
            "Secrets ctx  : (use `greentic-secrets ctx set --env <env> --tenant <tenant> [--team <team>]`)"
        ),
    };
    println!(
        "Seed/apply   : greentic-secrets init --pack fixtures/packs/messaging_secrets_smoke/pack.yaml --env {env} --tenant <tenant> --team <team> --non-interactive"
    );

    let registry = load_adapter_registry_for_cli(packs_root.clone(), &pack, no_default_packs)?;
    println!("\nAdapter packs:");
    println!(
        "  packs_root     : {}",
        packs_root
            .unwrap_or_else(|| PathBuf::from("packs"))
            .to_string_lossy()
    );
    if pack.is_empty() {
        println!("  extra packs    : (none)");
    } else {
        println!(
            "  extra packs    : {}",
            pack.iter()
                .map(|p| p.to_string_lossy())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    if registry.is_empty() {
        println!("  adapters       : (none loaded; add --pack or configure env)");
    } else {
        print_adapters(&registry, MessagingAdapterKind::Ingress, "ingress");
        print_adapters(&registry, MessagingAdapterKind::Egress, "egress");
        print_adapters(
            &registry,
            MessagingAdapterKind::IngressEgress,
            "ingress+egress",
        );
    }

    println!("\nAvailable services:");
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
        DevCommand::Down => {
            println!("Running make stack-down (best effort)…");
            if let Err(err) = run_make("stack-down", &[]) {
                eprintln!("stack-down failed: {err:?}");
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn handle_serve(
    kind: ServeKind,
    platform: String,
    tenant: String,
    team: Option<String>,
    pack: Vec<PathBuf>,
    packs_root: Option<PathBuf>,
    no_default_packs: bool,
    adapter_override: Option<String>,
) -> Result<()> {
    let env_scope = current_env();
    let team = team.unwrap_or_else(|| "default".into());
    let nats_url = env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
    let packs_root = packs_root.unwrap_or_else(|| PathBuf::from("packs"));

    let mut envs: Vec<(String, String)> = vec![
        ("GREENTIC_ENV".into(), env_scope.clone()),
        ("TENANT".into(), tenant.clone()),
        ("TEAM".into(), team.clone()),
        ("NATS_URL".into(), nats_url.clone()),
        (
            "MESSAGING_PACKS_ROOT".into(),
            packs_root.to_string_lossy().into(),
        ),
    ];
    if !pack.is_empty() {
        let joined = pack
            .iter()
            .map(|p| p.to_string_lossy())
            .collect::<Vec<_>>()
            .join(",");
        envs.push(("MESSAGING_ADAPTER_PACK_PATHS".into(), joined));
    }
    if no_default_packs {
        envs.push((
            "MESSAGING_INSTALL_ALL_DEFAULT_ADAPTER_PACKS".into(),
            "false".into(),
        ));
        envs.push(("MESSAGING_DEFAULT_ADAPTER_PACKS".into(), "".into()));
    }
    if let Some(adapter) = adapter_override {
        envs.push(("MESSAGING_EGRESS_ADAPTER".into(), adapter));
    }

    println!(
        "Starting {kind:?} {platform} (env={env_scope}, tenant={tenant}, team={team}, nats={nats_url}, packs_root={})",
        packs_root.to_string_lossy()
    );

    match kind {
        ServeKind::Ingress => {
            run_cargo_package_with_env("greentic-messaging", "gsm-gateway", &envs, &[])
        }
        ServeKind::Egress => {
            run_cargo_package_with_env("greentic-messaging", "gsm-egress", &envs, &[])
        }
        ServeKind::Subscriptions => run_cargo_package_with_env(
            "greentic-messaging",
            &subscription_package_for(&platform)?,
            &envs,
            &[],
        ),
    }
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

fn greentic_secrets_binary() -> String {
    env::var("GREENTIC_SECRETS_CLI").unwrap_or_else(|_| "greentic-secrets".into())
}

fn secrets_ctx() -> Result<Option<String>> {
    let bin = greentic_secrets_binary();
    let output = ProcessCommand::new(&bin)
        .args(["ctx", "show"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();
    match output {
        Ok(out) if out.status.success() => {
            let body = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if body.is_empty() {
                Ok(Some("(ctx configured, empty output)".into()))
            } else {
                Ok(Some(body))
            }
        }
        Ok(_) => Ok(None),
        Err(_) => Ok(None),
    }
}

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

fn subscription_package_for(platform: &str) -> Result<String> {
    let normalized = platform.to_ascii_lowercase();
    match normalized.as_str() {
        "teams" => Ok("gsm-subscriptions-teams".into()),
        _ => Err(anyhow!("subscriptions are only supported for: {}", "teams")),
    }
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

fn run_cargo_package_with_env(
    invoker: &str,
    package: &str,
    envs: &[(String, String)],
    args: &[&str],
) -> Result<()> {
    let args_vec: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    if cli_dry_run() {
        let env_prefix = if envs.is_empty() {
            String::new()
        } else {
            format!(
                "env {} ",
                envs.iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            )
        };
        println!(
            "(dry-run) {env_prefix}cargo run -p {package}{}",
            dry_suffix(&args_vec)
        );
        return Ok(());
    }
    let mut cmd = ProcessCommand::new("cargo");
    cmd.arg("run")
        .arg("-p")
        .arg(package)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    for (key, value) in envs {
        cmd.env(key, value);
    }
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

fn load_adapter_registry_for_cli(
    packs_root: Option<PathBuf>,
    extra_packs: &[PathBuf],
    no_default_packs: bool,
) -> Result<AdapterRegistry> {
    let packs_root = packs_root.unwrap_or_else(|| PathBuf::from("packs"));
    if !packs_root.exists() {
        // Create the directory so canonicalization in the registry loader succeeds.
        let _ = std::fs::create_dir_all(&packs_root);
    }
    let default_cfg = if no_default_packs {
        DefaultAdapterPacksConfig {
            install_all: false,
            selected: Vec::new(),
        }
    } else {
        DefaultAdapterPacksConfig::from_env()
    };
    let mut pack_paths = if no_default_packs {
        Vec::new()
    } else {
        default_adapter_pack_paths(packs_root.as_path(), &default_cfg)
    };
    pack_paths.extend(
        adapter_pack_paths_from_env()
            .into_iter()
            .filter_map(canonicalize_pack_path),
    );
    pack_paths.extend(
        extra_packs
            .iter()
            .filter_map(|p| canonicalize_pack_path(p.clone())),
    );

    AdapterRegistry::load_from_paths(packs_root.as_path(), &pack_paths).or_else(|err| {
        eprintln!(
            "warning: failed to load adapter packs (root={}, packs={:?}): {err}",
            packs_root.display(),
            pack_paths
        );
        Ok(AdapterRegistry::default())
    })
}

fn canonicalize_pack_path(path: PathBuf) -> Option<PathBuf> {
    std::fs::canonicalize(&path).ok().or_else(|| {
        eprintln!(
            "warning: could not canonicalize pack path {}; skipping",
            path.display()
        );
        None
    })
}

fn print_adapters(registry: &AdapterRegistry, kind: MessagingAdapterKind, label: &str) {
    let names: Vec<String> = registry.by_kind(kind).into_iter().map(|a| a.name).collect();
    if names.is_empty() {
        println!("  {label:<14}: (none)");
    } else {
        println!("  {label:<14}: {}", names.join(", "));
    }
}
