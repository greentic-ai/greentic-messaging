use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    env, fs,
    io::{self, BufRead, BufReader, Read, Seek, Write},
    path::{Path, PathBuf},
    process::{Child, Command as ProcessCommand, Stdio},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow};
use clap::{ArgAction, Parser, Subcommand, ValueEnum};
use greentic_config::ConfigResolver;
use greentic_config_types::{DevConfig, EnvId};
use greentic_pack::reader::{SigningPolicy, open_pack};
use greentic_types::{
    PackId, ProviderInstallId, TeamId, TenantCtx, TenantId,
    pack_manifest::{ExtensionInline, ExtensionRef, PackManifest},
    provider::{PROVIDER_EXTENSION_ID, ProviderExtensionInline},
};
use gsm_core::{
    AdapterDescriptor, AdapterRegistry, DefaultAdapterPacksConfig, MessagingAdapterKind, Platform,
    ProviderInstallState, default_adapter_pack_paths,
};
use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::str::FromStr;
use time::OffsetDateTime;

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
    #[command(hide = true)]
    Flows {
        #[command(subcommand)]
        command: FlowCommand,
    },
    /// Test wrappers for greentic-messaging-test
    Test {
        #[command(subcommand)]
        command: TestCommand,
    },
    /// Admin helpers (guard rails, Slack OAuth)
    Admin {
        #[command(subcommand)]
        command: AdminCommand,
    },
}

#[derive(Subcommand, Debug)]
enum DevCommand {
    /// Start gateway, runner, and egress for local development
    Up {
        /// Path to a messaging pack (.yaml or .gtpack); can be repeated.
        #[arg(long, value_name = "PATH")]
        pack: Vec<PathBuf>,
        /// Root directory that contains the packs folder (defaults to ./packs).
        #[arg(long, value_name = "PATH")]
        packs_root: Option<PathBuf>,
        /// Disable loading default packs shipped in packs/messaging.
        #[arg(long)]
        no_default_packs: bool,
        /// Tunnel provider for inbound webhooks.
        #[arg(long, value_enum, default_value_t = DevTunnel::Cloudflared)]
        tunnel: DevTunnel,
        /// Start subscription workers when supported.
        #[arg(long, default_value_t = true, action = ArgAction::Set)]
        subscriptions: bool,
    },
    /// Tail logs from local dev services
    Logs {
        /// Follow logs (default: true)
        #[arg(
            long,
            default_value_t = true,
            action = ArgAction::Set,
            num_args = 0..=1,
            default_missing_value = "true"
        )]
        #[arg(long = "no-follow", action = ArgAction::SetFalse)]
        follow: bool,
    },
    /// Configure a provider using greentic-config, greentic-secrets, and greentic-oauth
    Setup {
        provider: String,
        /// Override the environment (defaults to greentic-config dev.default_env).
        #[arg(long)]
        env: Option<String>,
        /// Override the tenant (defaults to greentic-config dev.default_tenant).
        #[arg(long)]
        tenant: Option<String>,
        /// Override the team (defaults to greentic-config dev.default_team).
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
        /// Update the existing install record if it exists.
        #[arg(long)]
        update: bool,
        /// Delete the install record instead of creating it.
        #[arg(long)]
        delete: bool,
        /// Optional install id override.
        #[arg(long)]
        install_id: Option<String>,
    },
    /// Stop and clean the local docker stack (stack-down)
    #[command(hide = true)]
    Down,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum DevTunnel {
    Cloudflared,
    None,
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
    #[command(hide = true)]
    Teams {
        #[command(subcommand)]
        command: TeamsAdminCommand,
    },
    #[command(hide = true)]
    Telegram {
        #[command(subcommand)]
        command: TelegramAdminCommand,
    },
    #[command(name = "whatsapp")]
    #[command(hide = true)]
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
        /// Legacy setup arguments (command disabled).
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
}

#[derive(Subcommand, Debug)]
enum TelegramAdminCommand {
    Setup {
        /// Legacy setup arguments (command disabled).
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
}

#[derive(Subcommand, Debug)]
enum WhatsAppAdminCommand {
    Setup {
        /// Legacy setup arguments (command disabled).
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
            "  adapters       : (none loaded; add --pack or configure env; packs must declare provider extension {})",
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
    println!("  runner/flows  : gsm-runner (pack-driven; uses MESSAGING_PACKS_ROOT + pack envs)\n");

    println!("For more commands, run `greentic-messaging --help`.");
    Ok(())
}

fn handle_dev(command: DevCommand) -> Result<()> {
    match command {
        DevCommand::Up {
            pack,
            packs_root,
            no_default_packs,
            tunnel,
            subscriptions,
        } => handle_dev_up(pack, packs_root, no_default_packs, tunnel, subscriptions),
        DevCommand::Logs { follow } => handle_dev_logs(follow),
        DevCommand::Setup {
            provider,
            env,
            tenant,
            team,
            pack,
            packs_root,
            no_default_packs,
            update,
            delete,
            install_id,
        } => handle_dev_setup(
            provider,
            env,
            tenant,
            team,
            pack,
            packs_root,
            no_default_packs,
            update,
            delete,
            install_id,
        ),
        DevCommand::Down => handle_dev_down(),
    }
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

const DEV_DIR: &str = ".greentic/dev";
const DEV_RUNTIME_FILE: &str = ".greentic/dev/runtime.json";
const DEV_INSTALLS_FILE: &str = ".greentic/dev/installs.json";
const DEFAULT_GATEWAY_PORT: u16 = 8080;

#[derive(Debug, Serialize, Deserialize)]
struct DevRuntime {
    env: String,
    public_base_url: Option<String>,
    logs: BTreeMap<String, String>,
    pids: BTreeMap<String, u32>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct DevInstallStore {
    #[serde(default)]
    records: Vec<greentic_types::ProviderInstallRecord>,
    #[serde(default)]
    states: Vec<ProviderInstallState>,
}

#[derive(Debug)]
struct DevContext {
    env: String,
    tenant: String,
    team: String,
}

#[derive(Debug, Default)]
struct DevContextOverrides {
    env: Option<String>,
    tenant: Option<String>,
    team: Option<String>,
}

fn resolve_dev_context(overrides: DevContextOverrides) -> Result<DevContext> {
    let dev_defaults = match ConfigResolver::new().load() {
        Ok(resolved) => resolved.config.dev.unwrap_or_else(default_dev_config),
        Err(err) => {
            eprintln!("warning: greentic-config load failed: {err:?}");
            default_dev_config()
        }
    };
    let env = overrides.env.unwrap_or(dev_defaults.default_env.0);
    let tenant = overrides.tenant.unwrap_or(dev_defaults.default_tenant);
    let team = overrides
        .team
        .or(dev_defaults.default_team)
        .unwrap_or_else(|| "default".into());
    Ok(DevContext { env, tenant, team })
}

fn default_dev_config() -> DevConfig {
    DevConfig {
        default_env: EnvId::try_from("dev").expect("valid env id"),
        default_tenant: "example".into(),
        default_team: None,
    }
}

fn ensure_dev_dir() -> Result<PathBuf> {
    let path = PathBuf::from(DEV_DIR);
    fs::create_dir_all(&path).with_context(|| format!("failed to create {}", path.display()))?;
    Ok(path)
}

fn dev_runtime_path() -> PathBuf {
    PathBuf::from(DEV_RUNTIME_FILE)
}

fn dev_installs_path() -> PathBuf {
    PathBuf::from(DEV_INSTALLS_FILE)
}

fn write_dev_installs(store: &DevInstallStore) -> Result<()> {
    let payload = serde_json::to_string_pretty(store).context("serialize install records")?;
    fs::write(dev_installs_path(), payload)
        .with_context(|| format!("failed to write {}", dev_installs_path().display()))
}

fn read_dev_installs() -> Result<DevInstallStore> {
    let path = dev_installs_path();
    let payload = match fs::read_to_string(&path) {
        Ok(payload) => payload,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Ok(DevInstallStore::default());
        }
        Err(err) => return Err(anyhow!("failed to read {}: {err}", path.display())),
    };
    let mut store: DevInstallStore =
        serde_json::from_str(&payload).context("invalid install records payload")?;
    if store.states.is_empty() {
        store.states = store
            .records
            .drain(..)
            .map(ProviderInstallState::new)
            .collect();
    }
    Ok(store)
}

fn upsert_install_record(
    store: &mut DevInstallStore,
    state: ProviderInstallState,
    allow_update: bool,
) -> Result<()> {
    if let Some(pos) = store
        .states
        .iter()
        .position(|existing| install_key_eq(&existing.record, &state.record))
    {
        if !allow_update {
            return Err(anyhow!(
                "install record already exists (use --update to overwrite)"
            ));
        }
        store.states[pos] = state;
    } else {
        store.states.push(state);
    }
    Ok(())
}

fn delete_install_record(
    store: &mut DevInstallStore,
    record: &greentic_types::ProviderInstallRecord,
) -> bool {
    let before = store.states.len();
    store
        .states
        .retain(|existing| !install_key_eq(&existing.record, record));
    before != store.states.len()
}

fn install_key_eq(
    left: &greentic_types::ProviderInstallRecord,
    right: &greentic_types::ProviderInstallRecord,
) -> bool {
    left.tenant.env == right.tenant.env
        && left.tenant.tenant == right.tenant.tenant
        && left.tenant.team == right.tenant.team
        && left.provider_id == right.provider_id
        && left.install_id == right.install_id
}

fn write_dev_runtime(runtime: &DevRuntime) -> Result<()> {
    let payload = serde_json::to_string_pretty(runtime)?;
    fs::write(dev_runtime_path(), payload)
        .with_context(|| format!("failed to write {}", dev_runtime_path().display()))
}

fn read_dev_runtime() -> Result<DevRuntime> {
    let payload = fs::read_to_string(dev_runtime_path())
        .with_context(|| format!("failed to read {}", dev_runtime_path().display()))?;
    serde_json::from_str(&payload).context("invalid dev runtime payload")
}

fn handle_dev_up(
    pack: Vec<PathBuf>,
    packs_root: Option<PathBuf>,
    no_default_packs: bool,
    tunnel: DevTunnel,
    subscriptions: bool,
) -> Result<()> {
    let context = resolve_dev_context(DevContextOverrides::default())?;
    let nats_url = env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
    let packs_root = packs_root.unwrap_or_else(|| PathBuf::from("packs"));

    let (_, pack_paths) =
        load_adapter_registry_for_cli(Some(packs_root.clone()), &pack, no_default_packs)?;

    ensure_stack_up()?;

    let gateway_port = env::var("MESSAGING_GATEWAY_PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(DEFAULT_GATEWAY_PORT);
    let local_base_url = format!("http://localhost:{gateway_port}");

    let dev_dir = ensure_dev_dir()?;
    let mut runtime = DevRuntime {
        env: context.env.clone(),
        public_base_url: None,
        logs: BTreeMap::new(),
        pids: BTreeMap::new(),
    };

    let (tunnel_child, public_base_url) = match tunnel {
        DevTunnel::Cloudflared => {
            let log_path = dev_dir.join("tunnel.log");
            runtime
                .logs
                .insert("tunnel".into(), log_path.to_string_lossy().into());
            if cli_dry_run() {
                println!("(dry-run) cloudflared tunnel --url {}", local_base_url);
                (None, Some(local_base_url.clone()))
            } else {
                match start_cloudflared_tunnel(gateway_port, &log_path) {
                    Ok((child, url)) => (Some(child), url.or_else(|| Some(local_base_url.clone()))),
                    Err(err) => {
                        eprintln!("cloudflared failed: {err:?}");
                        (None, Some(local_base_url.clone()))
                    }
                }
            }
        }
        DevTunnel::None => (None, Some(local_base_url.clone())),
    };

    let public_base_url = public_base_url.unwrap_or(local_base_url);
    runtime.public_base_url = Some(public_base_url.clone());

    let config = ServeEnvConfig {
        env_scope: context.env.clone(),
        tenant: context.tenant.clone(),
        team: context.team.clone(),
        nats_url,
        packs_root: packs_root.clone(),
        pack: pack.clone(),
        no_default_packs,
        adapter_override: None,
    };
    let mut envs = build_serve_envs(&config);
    envs.push(("PUBLIC_BASE_URL".into(), public_base_url.clone()));

    if cli_dry_run() {
        print_dry_run_cargo(&envs, "gsm-gateway", &[]);
        print_dry_run_cargo(&envs, "gsm-runner", &[]);
        print_dry_run_cargo(&envs, "gsm-egress", &[]);
        if subscriptions {
            print_dry_run_cargo(&envs, "gsm-subscriptions-teams", &[]);
        }
        return Ok(());
    }

    if let Some(child) = tunnel_child {
        runtime.pids.insert("tunnel".into(), child.id());
    }

    let gateway_log = dev_dir.join("gateway.log");
    runtime
        .logs
        .insert("gateway".into(), gateway_log.to_string_lossy().into());
    let gateway = spawn_cargo_package_with_env_logs("gsm-gateway", &envs, &[], &gateway_log)?;
    runtime.pids.insert("gateway".into(), gateway.id());

    let runner_log = dev_dir.join("runner.log");
    runtime
        .logs
        .insert("runner".into(), runner_log.to_string_lossy().into());
    let runner = spawn_cargo_package_with_env_logs("gsm-runner", &envs, &[], &runner_log)?;
    runtime.pids.insert("runner".into(), runner.id());

    let egress_log = dev_dir.join("egress.log");
    runtime
        .logs
        .insert("egress".into(), egress_log.to_string_lossy().into());
    let egress = spawn_cargo_package_with_env_logs("gsm-egress", &envs, &[], &egress_log)?;
    runtime.pids.insert("egress".into(), egress.id());

    if subscriptions {
        let extensions =
            gsm_core::load_provider_extensions_from_pack_files(packs_root.as_path(), &pack_paths)
                .unwrap_or_else(|_| gsm_core::ProviderExtensionsRegistry::default());
        if extensions.subscriptions.is_empty() {
            println!("Subscriptions: no providers declared; skipping.");
        } else {
            let package = subscription_package_for("subscriptions")?;
            if cli_dry_run() {
                print_dry_run_cargo(&envs, &package, &[]);
            } else {
                let subscriptions_log = dev_dir.join("subscriptions.log");
                runtime.logs.insert(
                    "subscriptions".into(),
                    subscriptions_log.to_string_lossy().into(),
                );
                let child =
                    spawn_cargo_package_with_env_logs(&package, &envs, &[], &subscriptions_log)?;
                runtime.pids.insert("subscriptions".into(), child.id());
            }
        }
    }

    write_dev_runtime(&runtime)?;
    println!("Dev stack started. PUBLIC_BASE_URL={public_base_url} (logs in {DEV_DIR})");
    Ok(())
}

fn handle_dev_logs(follow: bool) -> Result<()> {
    if cli_dry_run() {
        println!("(dry-run) tail logs under {DEV_DIR}");
        return Ok(());
    }

    let runtime = read_dev_runtime()?;
    let mut entries = Vec::new();
    for (name, path) in runtime.logs {
        entries.push((name, PathBuf::from(path)));
    }
    if entries.is_empty() {
        return Err(anyhow!(
            "no logs recorded in {}",
            dev_runtime_path().display()
        ));
    }
    tail_logs(entries, follow)
}

#[allow(clippy::too_many_arguments)]
fn handle_dev_setup(
    provider: String,
    env: Option<String>,
    tenant: Option<String>,
    team: Option<String>,
    pack: Vec<PathBuf>,
    packs_root: Option<PathBuf>,
    no_default_packs: bool,
    update: bool,
    delete: bool,
    install_id: Option<String>,
) -> Result<()> {
    let context = resolve_dev_context(DevContextOverrides { env, tenant, team })?;
    ensure_dev_dir()?;
    if delete && update {
        return Err(anyhow!("--delete cannot be combined with --update"));
    }
    let packs_root = packs_root.unwrap_or_else(|| PathBuf::from("packs"));
    let (_, pack_paths) =
        load_adapter_registry_for_cli(Some(packs_root.clone()), &pack, no_default_packs)?;
    if pack_paths.is_empty() {
        return Err(anyhow!(
            "no packs available; add --pack or configure default packs"
        ));
    }

    let (pack_path, extensions) =
        find_pack_with_provider(packs_root.as_path(), &pack_paths, &provider)?;

    let (pack_id, pack_version) = load_pack_identity(&pack_path)?;
    let install_id = install_id.unwrap_or_else(|| default_install_id(&provider));
    let public_base_url = resolve_public_base_url();
    let state = build_install_state(
        &context,
        &provider,
        &install_id,
        pack_id,
        pack_version,
        &public_base_url,
    )?;

    if delete {
        let mut store = read_dev_installs()?;
        let removed = delete_install_record(&mut store, &state.record);
        write_dev_installs(&store)?;
        if removed {
            println!("Deleted install record for {provider} ({install_id})");
        } else {
            println!("No install record found for {provider} ({install_id})");
        }
        return Ok(());
    }

    let secrets_cli = greentic_secrets_binary();
    run_cli_command(
        &secrets_cli,
        "greentic-secrets",
        &[
            "init".into(),
            "--pack".into(),
            pack_path.display().to_string(),
            "--env".into(),
            context.env.clone(),
            "--tenant".into(),
            context.tenant.clone(),
            "--team".into(),
            context.team.clone(),
        ],
    )?;

    if extensions.oauth.contains_key(&provider) {
        let oauth_admin =
            env::var("GREENTIC_OAUTH_ADMIN").unwrap_or_else(|_| "greentic-oauth-admin".into());
        run_cli_command(
            &oauth_admin,
            "greentic-oauth-admin",
            &[
                "start".into(),
                provider.clone(),
                "--tenant".into(),
                context.tenant.clone(),
            ],
        )?;
    }

    let plan = run_provision_setup(
        &provider,
        &pack_path,
        &context,
        &public_base_url,
        &install_id,
    )
    .unwrap_or_else(|err| {
        eprintln!("warning: setup plan unavailable: {err}");
        serde_json::Value::Null
    });

    let setup_args = vec![
        "packs".to_string(),
        "conformance".to_string(),
        "--setup-only".to_string(),
        "--public-base-url".to_string(),
        public_base_url.clone(),
        "--pack-path".to_string(),
        pack_path.display().to_string(),
        "--env".to_string(),
        context.env.clone(),
        "--tenant".to_string(),
        context.tenant.clone(),
        "--team".to_string(),
        context.team.clone(),
    ];
    run_messaging_test_cli_str(setup_args)?;

    let mut store = read_dev_installs()?;
    let mut state = state.clone();
    apply_provision_plan(&plan, &mut state);
    upsert_install_record(&mut store, state.clone(), update)?;
    write_dev_installs(&store)?;

    println!(
        "Setup complete for {provider} (install_id={install_id}, env={}, tenant={}, team={})",
        context.env, context.tenant, context.team
    );
    Ok(())
}

fn resolve_public_base_url() -> String {
    if let Ok(runtime) = read_dev_runtime()
        && let Some(url) = runtime.public_base_url
    {
        return url;
    }
    if let Ok(url) = env::var("PUBLIC_BASE_URL") {
        return url;
    }
    let gateway_port = env::var("MESSAGING_GATEWAY_PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(DEFAULT_GATEWAY_PORT);
    format!("http://localhost:{gateway_port}")
}

fn greentic_provision_binary() -> String {
    env::var("GREENTIC_PROVISION_CLI").unwrap_or_else(|_| "greentic-provision".into())
}

fn run_provision_setup(
    provider: &str,
    pack_path: &Path,
    _ctx: &DevContext,
    public_base_url: &str,
    install_id: &str,
) -> Result<serde_json::Value> {
    let bin = greentic_provision_binary();
    let args = vec![
        "dry-run".to_string(),
        "setup".to_string(),
        "--pack".to_string(),
        pack_path.display().to_string(),
        "--provider-id".to_string(),
        provider.to_string(),
        "--install-id".to_string(),
        install_id.to_string(),
        "--public-base-url".to_string(),
        public_base_url.to_string(),
        "--json".to_string(),
    ];
    run_cli_command_capture_json(&bin, "greentic-provision", &args)
}

fn load_pack_identity(path: &Path) -> Result<(PackId, Version)> {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase());
    if ext.as_deref() == Some("gtpack") {
        let manifest = decode_pack_manifest(&PathBuf::from(path))
            .ok_or_else(|| anyhow!("failed to decode pack manifest {}", path.display()))?;
        return Ok((manifest.pack_id, manifest.version));
    }
    #[derive(Deserialize)]
    struct PackSpec {
        id: String,
        version: String,
    }
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read pack file {}", path.display()))?;
    let spec: PackSpec = serde_yaml_bw::from_str(&raw)
        .with_context(|| format!("{} is not a valid pack spec", path.display()))?;
    let pack_id = PackId::new(&spec.id).context("invalid pack id")?;
    let version = Version::parse(&spec.version).context("invalid pack version")?;
    Ok((pack_id, version))
}

fn default_install_id(provider: &str) -> String {
    let mut out = String::from("dev-");
    for ch in provider.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push('-');
        }
    }
    out
}

fn build_install_state(
    ctx: &DevContext,
    provider: &str,
    install_id: &str,
    pack_id: PackId,
    pack_version: Version,
    public_base_url: &str,
) -> Result<ProviderInstallState> {
    let tenant = TenantCtx::new(
        ctx.env.parse::<EnvId>().context("invalid env id")?,
        ctx.tenant
            .parse::<TenantId>()
            .context("invalid tenant id")?,
    )
    .with_team(Some(ctx.team.parse::<TeamId>().context("invalid team id")?));
    let now = OffsetDateTime::now_utc();
    let record = greentic_types::ProviderInstallRecord {
        tenant,
        provider_id: provider.to_string(),
        install_id: install_id
            .parse::<ProviderInstallId>()
            .context("invalid install id")?,
        pack_id,
        pack_version,
        created_at: now,
        updated_at: now,
        config_refs: BTreeMap::new(),
        secret_refs: BTreeMap::new(),
        webhook_state: Value::Object(Default::default()),
        subscriptions_state: Value::Object(Default::default()),
        metadata: serde_json::json!({
            "public_base_url": public_base_url,
        }),
    };
    Ok(ProviderInstallState::new(record))
}

fn apply_provision_plan(plan: &serde_json::Value, state: &mut ProviderInstallState) {
    let Some(obj) = plan.as_object() else {
        return;
    };
    if let Some(Value::Object(config)) = obj.get("config") {
        for (key, value) in config {
            state.config.insert(key.clone(), value.clone());
            state
                .record
                .config_refs
                .entry(key.clone())
                .or_insert_with(|| format!("state:{key}"));
        }
    }
    if let Some(Value::Object(secrets)) = obj.get("secrets") {
        for (key, value) in secrets {
            let secret = value
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or_else(|| value.to_string());
            state.secrets.insert(key.clone(), secret);
            state
                .record
                .secret_refs
                .entry(key.clone())
                .or_insert_with(|| format!("secrets:{key}"));
        }
    }
    if let Some(Value::Object(config_refs)) = obj.get("config_refs") {
        for (key, value) in config_refs {
            if let Some(val) = value.as_str() {
                state
                    .record
                    .config_refs
                    .insert(key.clone(), val.to_string());
            }
        }
    }
    if let Some(Value::Object(secret_refs)) = obj.get("secret_refs") {
        for (key, value) in secret_refs {
            if let Some(val) = value.as_str() {
                state
                    .record
                    .secret_refs
                    .insert(key.clone(), val.to_string());
            }
        }
    }
    if let Some(Value::Object(webhook)) = obj.get("webhook_state").or_else(|| obj.get("webhook")) {
        state.record.webhook_state = Value::Object(webhook.clone());
    }
    if let Some(Value::Object(subs)) = obj
        .get("subscriptions_state")
        .or_else(|| obj.get("subscriptions"))
    {
        state.record.subscriptions_state = Value::Object(subs.clone());
    }
    if let Some(Value::Object(meta)) = obj.get("metadata") {
        if !state.record.metadata.is_object() {
            state.record.metadata = Value::Object(Default::default());
        }
        if let Some(target) = state.record.metadata.as_object_mut() {
            for (key, value) in meta {
                target.insert(key.clone(), value.clone());
            }
        }
    }
}

fn handle_dev_down() -> Result<()> {
    if cli_dry_run() {
        println!("(dry-run) stop dev processes and docker stack");
        ensure_stack_down()?;
        return Ok(());
    }

    if let Ok(runtime) = read_dev_runtime() {
        for (name, pid) in runtime.pids {
            let status = ProcessCommand::new("kill").arg(pid.to_string()).status();
            if let Err(err) = status {
                eprintln!("failed to stop {name} (pid {pid}): {err:?}");
            }
        }
        let _ = fs::remove_file(dev_runtime_path());
    }

    ensure_stack_down()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_provision_plan_populates_state() {
        let record = greentic_types::ProviderInstallRecord {
            tenant: TenantCtx::new("dev".parse().unwrap(), "acme".parse().unwrap()),
            provider_id: "messaging.test".into(),
            install_id: "install-a".parse().unwrap(),
            pack_id: "pack".parse().unwrap(),
            pack_version: Version::new(0, 1, 0),
            created_at: OffsetDateTime::now_utc(),
            updated_at: OffsetDateTime::now_utc(),
            config_refs: BTreeMap::new(),
            secret_refs: BTreeMap::new(),
            webhook_state: Value::Object(Default::default()),
            subscriptions_state: Value::Object(Default::default()),
            metadata: Value::Object(Default::default()),
        };
        let mut state = ProviderInstallState::new(record);
        let plan = serde_json::json!({
            "config": { "base_url": "https://example.invalid" },
            "secrets": { "token": "secret" },
            "config_refs": { "base_url": "state:base_url" },
            "secret_refs": { "token": "secrets:token" },
            "webhook_state": { "signature_header": "x-signature" },
            "subscriptions_state": { "last_sync": "now" },
            "metadata": { "routing": { "platform": "slack" } }
        });

        apply_provision_plan(&plan, &mut state);

        assert_eq!(
            state.config.get("base_url"),
            Some(&serde_json::json!("https://example.invalid"))
        );
        assert_eq!(state.secrets.get("token"), Some(&"secret".into()));
        assert_eq!(
            state.record.config_refs.get("base_url").map(String::as_str),
            Some("state:base_url")
        );
        assert_eq!(
            state.record.secret_refs.get("token").map(String::as_str),
            Some("secrets:token")
        );
        assert_eq!(
            state.record.webhook_state.get("signature_header"),
            Some(&serde_json::json!("x-signature"))
        );
        assert_eq!(
            state.record.subscriptions_state.get("last_sync"),
            Some(&serde_json::json!("now"))
        );
        assert!(state.record.metadata.get("routing").is_some());
    }
}

fn print_dry_run_cargo(envs: &[(String, String)], package: &str, args: &[&str]) {
    let arg_vec: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let env_prefix = format_env_prefix(envs);
    println!(
        "(dry-run) {env_prefix}cargo run -p {package}{}",
        dry_suffix(&arg_vec)
    );
}

fn format_env_prefix(envs: &[(String, String)]) -> String {
    if envs.is_empty() {
        String::new()
    } else {
        format!(
            "env {} ",
            envs.iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(" ")
        )
    }
}

fn spawn_cargo_package_with_env_logs(
    package: &str,
    envs: &[(String, String)],
    args: &[&str],
    log_path: &Path,
) -> Result<Child> {
    let log_file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .with_context(|| format!("failed to open {}", log_path.display()))?;
    let log_err = log_file.try_clone()?;
    let mut cmd = ProcessCommand::new("cargo");
    cmd.arg("run")
        .arg("-p")
        .arg(package)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(log_err));
    for (key, value) in envs {
        cmd.env(key, value);
    }
    if !args.is_empty() {
        cmd.arg("--");
        for arg in args {
            cmd.arg(arg);
        }
    }
    cmd.spawn()
        .with_context(|| format!("failed to spawn {package}"))
}

fn start_cloudflared_tunnel(port: u16, log_path: &Path) -> Result<(Child, Option<String>)> {
    let mut cmd = ProcessCommand::new("cloudflared");
    cmd.arg("tunnel")
        .arg("--url")
        .arg(format!("http://localhost:{port}"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().context("failed to start cloudflared tunnel")?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let log_file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .with_context(|| format!("failed to open {}", log_path.display()))?;
    let log_file = Arc::new(Mutex::new(log_file));
    let (tx, rx) = mpsc::channel::<String>();

    if let Some(stream) = stdout {
        spawn_tunnel_reader(stream, Arc::clone(&log_file), tx.clone());
    }
    if let Some(stream) = stderr {
        spawn_tunnel_reader(stream, Arc::clone(&log_file), tx.clone());
    }

    let url = rx.recv_timeout(Duration::from_secs(10)).ok();
    Ok((child, url))
}

fn spawn_tunnel_reader<R: Read + Send + 'static>(
    stream: R,
    log_file: Arc<Mutex<fs::File>>,
    tx: mpsc::Sender<String>,
) {
    thread::spawn(move || {
        let reader = BufReader::new(stream);
        for line in reader.lines().map_while(Result::ok) {
            if let Ok(mut file) = log_file.lock() {
                let _ = writeln!(file, "{line}");
            }
            if let Some(url) = extract_public_url(&line) {
                let _ = tx.send(url);
            }
        }
    });
}

fn extract_public_url(line: &str) -> Option<String> {
    for token in line.split_whitespace() {
        if token.starts_with("https://") {
            let cleaned =
                token.trim_matches(|ch: char| ch == '"' || ch == '\'' || ch == ',' || ch == ';');
            if cleaned.contains("trycloudflare.com") || cleaned.contains("cloudflare.com") {
                return Some(cleaned.to_string());
            }
        }
    }
    None
}

fn tail_logs(entries: Vec<(String, PathBuf)>, follow: bool) -> Result<()> {
    let mut handles = Vec::new();
    for (name, path) in entries {
        let handle = thread::spawn(move || {
            if follow {
                if let Err(err) = tail_log_file(&name, &path) {
                    eprintln!("log tail failed for {name}: {err:?}");
                }
            } else if let Err(err) = print_log_file(&name, &path) {
                eprintln!("log tail failed for {name}: {err:?}");
            }
        });
        handles.push(handle);
    }
    for handle in handles {
        let _ = handle.join();
    }
    Ok(())
}

fn tail_log_file(name: &str, path: &Path) -> Result<()> {
    let file = loop {
        match fs::OpenOptions::new().read(true).open(path) {
            Ok(file) => break file,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                thread::sleep(Duration::from_millis(250));
                continue;
            }
            Err(err) => return Err(anyhow!("cannot open log file {}: {err}", path.display())),
        }
    };
    let mut reader = BufReader::new(file);
    reader.seek(io::SeekFrom::End(0))?;
    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line)?;
        if read == 0 {
            thread::sleep(Duration::from_millis(250));
            continue;
        }
        print!("[{name}] {line}");
        io::stdout().flush().ok();
    }
}

fn print_log_file(name: &str, path: &Path) -> Result<()> {
    let file = fs::OpenOptions::new()
        .read(true)
        .open(path)
        .with_context(|| format!("cannot open log file {}", path.display()))?;
    let reader = BufReader::new(file);
    for line in reader.lines().map_while(Result::ok) {
        println!("[{name}] {line}");
    }
    Ok(())
}

fn find_pack_with_provider(
    packs_root: &Path,
    pack_paths: &[PathBuf],
    provider: &str,
) -> Result<(PathBuf, gsm_core::ProviderExtensionsRegistry)> {
    let mut matches = Vec::new();
    for path in pack_paths {
        let extensions = gsm_core::load_provider_extensions_from_pack_files(
            packs_root,
            std::slice::from_ref(path),
        )
        .unwrap_or_else(|_| gsm_core::ProviderExtensionsRegistry::default());
        let mut matched = extensions.ingress.contains_key(provider)
            || extensions.oauth.contains_key(provider)
            || extensions.subscriptions.contains_key(provider);
        if !matched {
            matched = decode_pack_manifest(path)
                .and_then(|manifest| provider_inline(&manifest))
                .is_some_and(|inline| {
                    inline
                        .providers
                        .iter()
                        .any(|decl| decl.provider_type == provider)
                });
        }
        if matched {
            matches.push((path.clone(), extensions));
        }
    }

    match matches.len() {
        0 => {
            let registry =
                gsm_core::load_provider_extensions_from_pack_files(packs_root, pack_paths)
                    .unwrap_or_else(|_| gsm_core::ProviderExtensionsRegistry::default());
            let mut providers = BTreeSet::new();
            providers.extend(registry.ingress.keys().cloned());
            providers.extend(registry.oauth.keys().cloned());
            providers.extend(registry.subscriptions.keys().cloned());
            for path in pack_paths {
                if let Some(manifest) = decode_pack_manifest(path)
                    && let Some(inline) = provider_inline(&manifest)
                {
                    for provider in inline.providers {
                        providers.insert(provider.provider_type);
                    }
                }
            }
            let providers = providers.into_iter().collect::<Vec<_>>();
            Err(anyhow!(
                "provider {provider} not found in packs (available: {})",
                providers.join(", ")
            ))
        }
        1 => Ok(matches.remove(0)),
        _ => {
            let paths = matches
                .iter()
                .map(|(path, _)| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            Err(anyhow!(
                "provider {provider} found in multiple packs: {paths}"
            ))
        }
    }
}

fn run_cli_command(binary: &str, display: &str, args: &[String]) -> Result<()> {
    if cli_dry_run() {
        println!("(dry-run) {display} [args redacted: {}]", args.len());
        return Ok(());
    }
    let status = ProcessCommand::new(binary)
        .args(args)
        .status()
        .with_context(|| format!("failed to run {binary}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("{binary} exited with status {status}"))
    }
}

fn run_cli_command_capture_json(
    binary: &str,
    display: &str,
    args: &[String],
) -> Result<serde_json::Value> {
    if cli_dry_run() {
        println!("(dry-run) {display} [args redacted: {}]", args.len());
        return Ok(serde_json::Value::Null);
    }
    let output = ProcessCommand::new(binary)
        .args(args)
        .output()
        .with_context(|| format!("failed to run {binary}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!(
            "{binary} exited with status {}: {stderr}",
            output.status
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&stdout).with_context(|| format!("invalid JSON output: {}", stdout.trim()))
}

fn handle_flows(command: FlowCommand) -> Result<()> {
    match command {
        FlowCommand::Run {
            flow,
            platform,
            tenant,
            team,
        } => {
            let platform = Platform::from_str(&platform)
                .map_err(|err| anyhow!("invalid platform {platform}: {err}"))?;
            run_flow_with_runner(flow, platform, tenant, team)
        }
    }
}

fn run_flow_with_runner(
    _flow: PathBuf,
    _platform: Platform,
    _tenant: String,
    _team: Option<String>,
) -> Result<()> {
    Err(legacy_disabled_error())
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
        AdminCommand::Teams { command } => handle_teams_admin(command),
        AdminCommand::Telegram { command } => handle_telegram_admin(command),
        AdminCommand::WhatsApp { command } => handle_whatsapp_admin(command),
    }
}

fn handle_teams_admin(command: TeamsAdminCommand) -> Result<()> {
    let _ = command;
    Err(legacy_disabled_error())
}

fn handle_telegram_admin(command: TelegramAdminCommand) -> Result<()> {
    let _ = command;
    Err(legacy_disabled_error())
}

fn handle_whatsapp_admin(command: WhatsAppAdminCommand) -> Result<()> {
    let _ = command;
    Err(legacy_disabled_error())
}

const LEGACY_DISABLED_MESSAGE: &str = "Legacy messaging is disabled. Use `messaging dev up`.";

fn legacy_disabled_error() -> anyhow::Error {
    anyhow!(LEGACY_DISABLED_MESSAGE)
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

const STACK_PROJECT: &str = "greentic-messaging-dev";
const EMBEDDED_STACK_YAML: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../docker/stack.yml"
));

#[derive(Copy, Clone, Debug)]
enum StackCommand {
    Up,
    Down,
}

fn makefile_available() -> bool {
    Path::new("Makefile").exists()
}

fn ensure_stack_up() -> Result<()> {
    if makefile_available() {
        println!("Starting docker stack (best effort)…");
        if let Err(err) = run_make("stack-up", &[]) {
            eprintln!("stack-up failed: {err:?}");
            println!("Falling back to embedded docker compose stack…");
            if let Err(err) = run_embedded_stack(StackCommand::Up) {
                eprintln!("embedded stack-up failed: {err:?}");
            }
            return Ok(());
        }
        Ok(())
    } else {
        println!("Starting embedded docker compose stack…");
        if let Err(err) = run_embedded_stack(StackCommand::Up) {
            eprintln!("embedded stack-up failed: {err:?}");
        }
        Ok(())
    }
}

fn ensure_stack_down() -> Result<()> {
    if makefile_available() {
        println!("Stopping docker stack (best effort)…");
        if let Err(err) = run_make("stack-down", &[]) {
            eprintln!("stack-down failed: {err:?}");
            println!("Falling back to embedded docker compose stack…");
            if let Err(err) = run_embedded_stack(StackCommand::Down) {
                eprintln!("embedded stack-down failed: {err:?}");
            }
            return Ok(());
        }
        Ok(())
    } else {
        println!("Stopping embedded docker compose stack…");
        if let Err(err) = run_embedded_stack(StackCommand::Down) {
            eprintln!("embedded stack-down failed: {err:?}");
        }
        Ok(())
    }
}

fn run_embedded_stack(command: StackCommand) -> Result<()> {
    if cli_dry_run() {
        println!(
            "(dry-run) docker compose -p {STACK_PROJECT} -f <embedded> {}",
            match command {
                StackCommand::Up => "up -d",
                StackCommand::Down => "down -v",
            }
        );
        return Ok(());
    }

    let stack_path = write_embedded_stack()?;
    let mut cmd = ProcessCommand::new("docker");
    cmd.arg("compose")
        .arg("-p")
        .arg(STACK_PROJECT)
        .arg("-f")
        .arg(&stack_path)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    match command {
        StackCommand::Up => {
            cmd.arg("up").arg("-d");
        }
        StackCommand::Down => {
            cmd.arg("down").arg("-v");
        }
    }

    let status = cmd
        .status()
        .with_context(|| format!("failed to run docker compose ({STACK_PROJECT})"))?;
    let _ = std::fs::remove_file(&stack_path);
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!(
            "docker compose {} exited with status {status}",
            match command {
                StackCommand::Up => "up",
                StackCommand::Down => "down",
            }
        ))
    }
}

fn write_embedded_stack() -> Result<PathBuf> {
    let mut path = env::temp_dir();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    path.push(format!("greentic-messaging-stack-{stamp}.yml"));
    std::fs::write(&path, EMBEDDED_STACK_YAML)
        .with_context(|| format!("failed to write embedded stack to {}", path.display()))?;
    Ok(path)
}

fn subscription_package_for(platform: &str) -> Result<String> {
    let _ = platform;
    Ok("gsm-subscriptions-teams".into())
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
        default_packs_from_env()
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

fn default_packs_from_env() -> DefaultAdapterPacksConfig {
    let install_all = env::var("MESSAGING_INSTALL_ALL_DEFAULT_ADAPTER_PACKS")
        .map(|v| v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let selected = env::var("MESSAGING_DEFAULT_ADAPTER_PACKS")
        .ok()
        .map(|raw| {
            raw.split(',')
                .filter_map(|s| {
                    let trimmed = s.trim();
                    if trimmed.is_empty() {
                        None
                    } else {
                        Some(trimmed.to_string())
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    DefaultAdapterPacksConfig::from_settings(install_all, selected)
}

fn adapter_pack_paths_from_env() -> Vec<PathBuf> {
    env::var("MESSAGING_ADAPTER_PACK_PATHS")
        .ok()
        .map(|raw| {
            raw.split(',')
                .filter_map(|s| {
                    let trimmed = s.trim();
                    if trimmed.is_empty() {
                        None
                    } else {
                        Some(PathBuf::from(trimmed))
                    }
                })
                .collect()
        })
        .unwrap_or_default()
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
