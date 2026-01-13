use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    env,
    io::Read,
    path::PathBuf,
    process::{Command as ProcessCommand, Stdio},
};

use anyhow::{Context, Result, anyhow};
use clap::{Parser, Subcommand, ValueEnum};
use greentic_pack::reader::{SigningPolicy, open_pack};
use greentic_types::{
    pack_manifest::{ExtensionInline, ExtensionRef, PackManifest},
    provider::{PROVIDER_EXTENSION_ID, ProviderExtensionInline},
};
use gsm_core::{
    AdapterDescriptor, AdapterRegistry, DefaultAdapterPacksConfig, MessagingAdapterKind, Platform,
    adapter_pack_paths_from_env, default_adapter_pack_paths,
};
use serde::Deserialize;
use std::str::FromStr;

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
    /// Serve ingress/egress/subscriptions or auto-start from packs
    Serve {
        #[arg(value_enum)]
        kind: ServeKind,
        #[arg(value_name = "PLATFORM")]
        platform: Option<String>,
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
        /// Arguments forwarded to `cargo run --manifest-path legacy/scripts/Cargo.toml --bin teams_setup -- ...`
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
}

#[derive(Subcommand, Debug)]
enum TelegramAdminCommand {
    Setup {
        /// Arguments forwarded to `cargo run --manifest-path legacy/scripts/Cargo.toml --bin telegram_setup -- ...`
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
}

#[derive(Subcommand, Debug)]
enum WhatsAppAdminCommand {
    Setup {
        /// Arguments forwarded to `cargo run --manifest-path legacy/scripts/Cargo.toml --bin whatsapp_setup -- ...`
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum ServeKind {
    Ingress,
    Egress,
    Subscriptions,
    Pack,
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
        "Seed/apply : greentic-secrets init --pack messaging-<name>.gtpack --env {env} --tenant <tenant> --team <team> --non-interactive"
    );

    let (registry, pack_paths) =
        load_adapter_registry_for_cli(packs_root.clone(), &pack, no_default_packs)?;
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
    let (flows, provider_flow_hints) = collect_pack_flows_and_provider_hints(&pack_paths);

    if registry.is_empty() {
        println!(
            "  adapters       : (none loaded; add --pack or configure env; packs must declare provider extension {} or legacy messaging.adapters)",
            greentic_types::provider::PROVIDER_EXTENSION_ID
        );
    } else {
        print_adapters(
            &registry,
            MessagingAdapterKind::Ingress,
            "ingress",
            &provider_flow_hints,
        );
        print_adapters(
            &registry,
            MessagingAdapterKind::Egress,
            "egress",
            &provider_flow_hints,
        );
        print_adapters(
            &registry,
            MessagingAdapterKind::IngressEgress,
            "ingress+egress",
            &provider_flow_hints,
        );
    }

    println!("\nFlows in loaded packs:");
    if flows.is_empty() {
        println!("  (none)");
    } else {
        for pack in flows {
            if pack.flows.is_empty() {
                println!("  {}: (none)", pack.label);
                continue;
            }
            println!("  {}:", pack.label);
            for flow in pack.flows {
                if let Some(title) = flow.title {
                    println!("    - {}  kind={}  {}", flow.id, flow.kind, title);
                } else {
                    println!("    - {}  kind={}", flow.id, flow.kind);
                }
            }
        }
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
    platform: Option<String>,
    tenant: String,
    team: Option<String>,
    pack: Vec<PathBuf>,
    packs_root: Option<PathBuf>,
    no_default_packs: bool,
    adapter_override: Option<String>,
) -> Result<()> {
    let platform = match kind {
        ServeKind::Pack => None,
        _ => Some(platform.ok_or_else(|| anyhow!("missing platform (e.g. slack, teams)"))?),
    };
    let env_scope = current_env();
    let team = team.unwrap_or_else(|| "default".into());
    let nats_url = env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
    let packs_root = packs_root.unwrap_or_else(|| PathBuf::from("packs"));

    let config = ServeEnvConfig {
        env_scope,
        tenant,
        team,
        nats_url,
        packs_root,
        pack,
        no_default_packs,
        adapter_override,
    };
    let envs = build_serve_envs(&config);

    match kind {
        ServeKind::Pack => handle_serve_pack(&config, envs),
        ServeKind::Ingress => {
            let platform = platform.expect("platform required");
            println!(
                "Starting {kind:?} {platform} (env={}, tenant={}, team={}, nats={}, packs_root={})",
                config.env_scope,
                config.tenant,
                config.team,
                config.nats_url,
                config.packs_root.to_string_lossy()
            );
            run_cargo_package_with_env("greentic-messaging", "gsm-gateway", &envs, &[])
        }
        ServeKind::Egress => {
            let platform = platform.expect("platform required");
            println!(
                "Starting {kind:?} {platform} (env={}, tenant={}, team={}, nats={}, packs_root={})",
                config.env_scope,
                config.tenant,
                config.team,
                config.nats_url,
                config.packs_root.to_string_lossy()
            );
            run_cargo_package_with_env("greentic-messaging", "gsm-egress", &envs, &[])
        }
        ServeKind::Subscriptions => {
            let platform = platform.expect("platform required");
            println!(
                "Starting {kind:?} {platform} (env={}, tenant={}, team={}, nats={}, packs_root={})",
                config.env_scope,
                config.tenant,
                config.team,
                config.nats_url,
                config.packs_root.to_string_lossy()
            );
            run_cargo_package_with_env(
                "greentic-messaging",
                &subscription_package_for(&platform)?,
                &envs,
                &[],
            )
        }
    }
}

struct ServeEnvConfig {
    env_scope: String,
    tenant: String,
    team: String,
    nats_url: String,
    packs_root: PathBuf,
    pack: Vec<PathBuf>,
    no_default_packs: bool,
    adapter_override: Option<String>,
}

fn handle_serve_pack(config: &ServeEnvConfig, envs: Vec<(String, String)>) -> Result<()> {
    let (registry, pack_paths) = load_adapter_registry_for_cli(
        Some(config.packs_root.clone()),
        &config.pack,
        config.no_default_packs,
    )?;
    if registry.is_empty() {
        return Err(anyhow!(
            "no adapters loaded; add --pack or configure env for adapter packs"
        ));
    }

    let (platforms, unknown_providers) = platforms_from_pack_paths(&pack_paths);
    let mut has_ingress = false;
    let mut has_egress = false;

    for adapter in registry.all() {
        match adapter.kind {
            MessagingAdapterKind::Ingress => has_ingress = true,
            MessagingAdapterKind::Egress => has_egress = true,
            MessagingAdapterKind::IngressEgress => {
                has_ingress = true;
                has_egress = true;
            }
        }
    }

    if !unknown_providers.is_empty() {
        eprintln!(
            "warning: could not infer platform for providers: {}",
            unknown_providers.join(", ")
        );
    }

    let mut packages: Vec<&str> = Vec::new();
    if has_ingress {
        packages.push("gsm-gateway");
    }
    if has_egress {
        packages.push("gsm-egress");
    }
    if platforms.iter().any(|platform| platform == "teams") {
        packages.push("gsm-subscriptions-teams");
    }

    if packages.is_empty() {
        return Err(anyhow!("no services selected from pack adapters"));
    }

    let platform_list = if platforms.is_empty() {
        "(unknown)".to_string()
    } else {
        platforms.iter().cloned().collect::<Vec<_>>().join(", ")
    };

    println!(
        "Starting pack services [{}] (platforms={platform_list}, env={}, tenant={}, team={}, nats={}, packs_root={})",
        packages.join(", "),
        config.env_scope,
        config.tenant,
        config.team,
        config.nats_url,
        config.packs_root.to_string_lossy()
    );

    run_cargo_packages_with_env("greentic-messaging", &packages, &envs, &[])
}

fn build_serve_envs(config: &ServeEnvConfig) -> Vec<(String, String)> {
    let mut envs: Vec<(String, String)> = vec![
        ("GREENTIC_ENV".into(), config.env_scope.clone()),
        ("TENANT".into(), config.tenant.clone()),
        ("TEAM".into(), config.team.clone()),
        ("NATS_URL".into(), config.nats_url.clone()),
        (
            "MESSAGING_PACKS_ROOT".into(),
            config.packs_root.to_string_lossy().into(),
        ),
    ];
    if !config.pack.is_empty() {
        let joined = config
            .pack
            .iter()
            .map(|p| p.to_string_lossy())
            .collect::<Vec<_>>()
            .join(",");
        envs.push(("MESSAGING_ADAPTER_PACK_PATHS".into(), joined));
    }
    if config.no_default_packs {
        envs.push((
            "MESSAGING_INSTALL_ALL_DEFAULT_ADAPTER_PACKS".into(),
            "false".into(),
        ));
        envs.push(("MESSAGING_DEFAULT_ADAPTER_PACKS".into(), "".into()));
    }
    if let Some(adapter) = config.adapter_override.clone() {
        envs.push(("MESSAGING_EGRESS_ADAPTER".into(), adapter));
    }
    envs
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
    run_cargo_package("greentic-messaging", "gsm-slack-oauth", &args)
}

fn run_teams_setup(args: Vec<String>) -> Result<()> {
    println!("Running Teams setup helper...");
    run_cargo_manifest_bin("legacy/scripts/Cargo.toml", "teams_setup", &args)
}

fn run_telegram_setup(args: Vec<String>) -> Result<()> {
    println!("Running Telegram setup helper...");
    run_cargo_manifest_bin("legacy/scripts/Cargo.toml", "telegram_setup", &args)
}

fn run_whatsapp_setup(args: Vec<String>) -> Result<()> {
    println!("Running WhatsApp setup helper...");
    run_cargo_manifest_bin("legacy/scripts/Cargo.toml", "whatsapp_setup", &args)
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

fn run_cargo_packages_with_env(
    invoker: &str,
    packages: &[&str],
    envs: &[(String, String)],
    args: &[&str],
) -> Result<()> {
    let args_vec: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    if cli_dry_run() {
        for package in packages {
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
        }
        return Ok(());
    }

    let mut children = Vec::new();
    for package in packages {
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
        let child = cmd.spawn().with_context(|| {
            format!("failed to run cargo package {package} (invoked by {invoker})")
        })?;
        children.push((*package, child));
    }

    let mut failures = Vec::new();
    for (package, mut child) in children {
        let status = child
            .wait()
            .with_context(|| format!("failed while waiting for {package}"))?;
        if !status.success() {
            failures.push(format!("{package} exited with status {status}"));
        }
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(anyhow!(failures.join("; ")))
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
) -> Result<(AdapterRegistry, Vec<PathBuf>)> {
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
    pack_paths = dedupe_pack_paths(pack_paths);

    let registry = match AdapterRegistry::load_from_paths(packs_root.as_path(), &pack_paths) {
        Ok(r) => r,
        Err(err) => {
            eprintln!(
                "warning: failed to load adapter packs (root={}, packs={:?}): {err}",
                packs_root.display(),
                pack_paths
            );
            AdapterRegistry::default()
        }
    };
    Ok((registry, pack_paths))
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

fn dedupe_pack_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for path in paths {
        let key = path.to_string_lossy().into_owned();
        if seen.insert(key) {
            out.push(path);
        }
    }
    out
}

fn print_adapters(
    registry: &AdapterRegistry,
    kind: MessagingAdapterKind,
    label: &str,
    provider_flow_hints: &ProviderFlowHintsByProvider,
) {
    let adapters = registry.by_kind(kind);
    if adapters.is_empty() {
        println!("  {label:<14}: (none)");
    } else {
        println!("  {label:<14}:");
        for adapter in adapters {
            println!("    - {}", format_adapter(&adapter));
            if let Some(hints) = provider_flow_hints.get(&adapter.name) {
                for hint in hints {
                    println!("      provider flows (from pack {}):", hint.pack_label);
                    for flow in &hint.hints {
                        if flow.missing {
                            println!(
                                "        {key:<18}: {id} (missing)",
                                key = flow.key,
                                id = flow.flow_id
                            );
                        } else {
                            println!("        {key:<18}: {id}", key = flow.key, id = flow.flow_id);
                        }
                    }
                }
            }
        }
    }
}

fn format_adapter(adapter: &AdapterDescriptor) -> String {
    let mut parts = Vec::new();
    parts.push(format!("component={}", adapter.component));
    if let Some(flow) = adapter.flow_path() {
        parts.push(format!("flow={}", flow));
    }
    if let Some(caps) = &adapter.capabilities {
        if !caps.direction.is_empty() {
            parts.push(format!("direction={}", caps.direction.join("|")));
        }
        if !caps.features.is_empty() {
            parts.push(format!("features={}", caps.features.join("|")));
        }
    }
    if parts.is_empty() {
        adapter.name.clone()
    } else {
        format!("{} ({})", adapter.name, parts.join(", "))
    }
}

#[derive(Debug)]
struct PackFlowsSummary {
    label: String,
    flows: Vec<FlowSummary>,
}

#[derive(Debug)]
struct FlowSummary {
    id: String,
    kind: String,
    title: Option<String>,
}

#[derive(Debug)]
struct ProviderFlowHintLine {
    key: &'static str,
    flow_id: String,
    missing: bool,
}

#[derive(Debug)]
struct ProviderFlowHints {
    pack_label: String,
    hints: Vec<ProviderFlowHintLine>,
}

type ProviderFlowHintsByProvider = BTreeMap<String, Vec<ProviderFlowHints>>;

fn collect_pack_flows_and_provider_hints(
    pack_paths: &[PathBuf],
) -> (Vec<PackFlowsSummary>, ProviderFlowHintsByProvider) {
    let mut summaries = Vec::new();
    let mut provider_hints: ProviderFlowHintsByProvider = BTreeMap::new();

    for path in pack_paths {
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.to_ascii_lowercase());
        if ext.as_deref() != Some("gtpack") {
            continue;
        }
        if let Some(manifest) = decode_pack_manifest(path) {
            let label = manifest.pack_id.to_string();

            let mut flows: Vec<FlowSummary> = manifest
                .flows
                .iter()
                .map(|f| FlowSummary {
                    id: f.id.to_string(),
                    kind: format!("{:?}", f.kind),
                    title: f.flow.metadata.title.clone(),
                })
                .collect();
            flows.sort_by(|a, b| a.id.cmp(&b.id));
            let flow_ids: HashSet<String> =
                manifest.flows.iter().map(|f| f.id.to_string()).collect();
            summaries.push(PackFlowsSummary {
                label: label.clone(),
                flows,
            });

            if let Some(hints) =
                extract_provider_flow_hints(manifest.extensions.as_ref(), &flow_ids, &label)
            {
                for (provider, hint) in hints {
                    provider_hints.entry(provider).or_default().push(hint);
                }
            }
        } else if let Ok(pack) = open_pack(path, SigningPolicy::DevOk) {
            let label = pack.manifest.meta.pack_id.clone();
            let mut flows: Vec<FlowSummary> = pack
                .manifest
                .flows
                .iter()
                .map(|f| FlowSummary {
                    id: f.id.clone(),
                    kind: f.kind.clone(),
                    title: None,
                })
                .collect();
            flows.sort_by(|a, b| a.id.cmp(&b.id));
            summaries.push(PackFlowsSummary { label, flows });
        }
    }

    summaries.sort_by(|a, b| a.label.cmp(&b.label));
    for hints in provider_hints.values_mut() {
        hints.sort_by(|a, b| a.pack_label.cmp(&b.pack_label));
    }
    (summaries, provider_hints)
}

fn decode_pack_manifest(path: &PathBuf) -> Option<greentic_types::pack_manifest::PackManifest> {
    let file = std::fs::File::open(path).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;
    let mut buf = Vec::new();
    archive
        .by_name("manifest.cbor")
        .ok()?
        .read_to_end(&mut buf)
        .ok()?;
    greentic_types::decode_pack_manifest(&buf).ok()
}

fn platforms_from_pack_paths(pack_paths: &[PathBuf]) -> (BTreeSet<String>, Vec<String>) {
    let mut platforms = BTreeSet::new();
    let mut unknown = BTreeSet::new();
    for path in pack_paths {
        if let Some(manifest) = decode_pack_manifest(path) {
            let (pack_platforms, pack_unknown) = platforms_from_manifest(&manifest);
            platforms.extend(pack_platforms);
            unknown.extend(pack_unknown);
        }
    }
    (platforms, unknown.into_iter().collect())
}

fn platforms_from_manifest(manifest: &PackManifest) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut platforms = BTreeSet::new();
    let mut unknown = BTreeSet::new();
    let Some(inline) = provider_inline(manifest) else {
        return (platforms, unknown);
    };
    for provider in inline.providers {
        if let Some(platform) = infer_platform_from_provider_type(&provider.provider_type) {
            platforms.insert(platform);
        } else {
            unknown.insert(provider.provider_type);
        }
    }
    (platforms, unknown)
}

fn infer_platform_from_provider_type(provider_type: &str) -> Option<String> {
    provider_type
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .find_map(|token| {
            Platform::from_str(token)
                .ok()
                .map(|p| p.as_str().to_string())
        })
}

fn provider_inline(manifest: &PackManifest) -> Option<ProviderExtensionInline> {
    manifest.provider_extension_inline().cloned().or_else(|| {
        manifest
            .extensions
            .as_ref()
            .and_then(|exts| exts.get(PROVIDER_EXTENSION_ID))
            .and_then(|ext| ext.inline.as_ref())
            .and_then(|inline| match inline {
                ExtensionInline::Provider(p) => Some(p.clone()),
                _ => None,
            })
    })
}

#[derive(Debug, Deserialize)]
struct ProviderFlowHintsPayload {
    #[serde(flatten)]
    providers: BTreeMap<String, ProviderFlowHintSet>,
}

#[derive(Debug, Deserialize)]
struct ProviderFlowHintSet {
    setup_default: Option<String>,
    setup_custom: Option<String>,
    reconfigure: Option<String>,
    diagnostics: Option<String>,
    verify_webhooks: Option<String>,
    rotate_credentials: Option<String>,
}

fn extract_provider_flow_hints(
    extensions: Option<&BTreeMap<String, ExtensionRef>>,
    flow_ids: &HashSet<String>,
    pack_label: &str,
) -> Option<BTreeMap<String, ProviderFlowHints>> {
    let extensions = extensions?;
    let ref_entry = extensions.get("messaging.provider_flow_hints")?;
    let inline = ref_entry.inline.as_ref()?;
    let payload = match inline {
        ExtensionInline::Other(value) => {
            serde_json::from_value::<ProviderFlowHintsPayload>(value.clone()).ok()?
        }
        _ => return None,
    };

    let mut out = BTreeMap::new();
    for (provider_id, hint_set) in payload.providers {
        let mut hints = Vec::new();
        for (key, value) in [
            ("setup_default", &hint_set.setup_default),
            ("setup_custom", &hint_set.setup_custom),
            ("reconfigure", &hint_set.reconfigure),
            ("diagnostics", &hint_set.diagnostics),
            ("verify_webhooks", &hint_set.verify_webhooks),
            ("rotate_credentials", &hint_set.rotate_credentials),
        ] {
            if let Some(flow_id) = value {
                hints.push(ProviderFlowHintLine {
                    key,
                    flow_id: flow_id.clone(),
                    missing: !flow_ids.contains(flow_id),
                });
            }
        }
        if !hints.is_empty() {
            out.insert(
                provider_id,
                ProviderFlowHints {
                    pack_label: pack_label.to_string(),
                    hints,
                },
            );
        }
    }
    if out.is_empty() { None } else { Some(out) }
}
