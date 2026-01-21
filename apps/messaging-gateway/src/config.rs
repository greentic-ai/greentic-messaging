use std::fs;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::str::FromStr;

use anyhow::{Context, Result, bail};
use greentic_config::ConfigResolver;
use greentic_config_types::{GreenticConfig, ServiceTransportConfig};
use greentic_types::EnvId;
use gsm_core::DefaultAdapterPacksConfig;

#[derive(Debug, Clone)]
pub struct GatewayConfig {
    pub env: EnvId,
    pub nats_url: String,
    pub addr: SocketAddr,
    pub default_team: String,
    pub subject_prefix: String,
    pub worker_routing: Option<gsm_core::WorkerRoutingConfig>,
    pub worker_routes: std::collections::BTreeMap<String, gsm_core::WorkerRoutingConfig>,
    pub packs_root: PathBuf,
    pub default_packs: DefaultAdapterPacksConfig,
    pub extra_pack_paths: Vec<PathBuf>,
    pub install_store_path: Option<PathBuf>,
}

impl GatewayConfig {
    pub fn load() -> Result<Self> {
        let resolved = ConfigResolver::new().load()?;
        Self::from_config(&resolved.config)
    }

    pub fn from_config(config: &GreenticConfig) -> Result<Self> {
        let env = config.environment.env_id.clone();
        let (nats_url, subject_prefix_override) =
            nats_settings(config)?.unwrap_or_else(|| ("nats://127.0.0.1:4222".to_string(), None));
        let (bind_addr, port) = gateway_bind_addr(config);
        let ip = IpAddr::from_str(&bind_addr).context("invalid gateway bind addr")?;
        let worker_routes = std::collections::BTreeMap::new();
        let worker_routing = None;
        let packs_root = config.paths.greentic_root.join("packs");
        Ok(Self {
            env,
            nats_url,
            addr: SocketAddr::new(ip, port),
            default_team: config
                .dev
                .as_ref()
                .and_then(|dev| dev.default_team.clone())
                .unwrap_or_else(|| "default".into()),
            subject_prefix: subject_prefix_override
                .unwrap_or_else(|| gsm_core::INGRESS_SUBJECT_PREFIX.to_string()),
            worker_routing,
            worker_routes,
            packs_root,
            default_packs: DefaultAdapterPacksConfig::default(),
            extra_pack_paths: Vec::new(),
            install_store_path: install_store_path(config),
        })
    }
}

fn gateway_bind_addr(config: &GreenticConfig) -> (String, u16) {
    let service = config
        .services
        .as_ref()
        .and_then(|services| services.source.as_ref())
        .and_then(|svc| svc.service.as_ref());
    let addr = service
        .and_then(|svc| svc.bind_addr.as_deref())
        .unwrap_or("0.0.0.0")
        .to_string();
    let port = service.and_then(|svc| svc.port).unwrap_or(8080);
    (addr, port)
}

fn nats_settings(config: &GreenticConfig) -> Result<Option<(String, Option<String>)>> {
    let transport = config
        .services
        .as_ref()
        .and_then(|services| services.events_transport.as_ref())
        .and_then(|svc| svc.transport.as_ref());
    match transport {
        Some(ServiceTransportConfig::Nats {
            url,
            subject_prefix,
        }) => Ok(Some((url.to_string(), subject_prefix.clone()))),
        Some(ServiceTransportConfig::Http { .. }) => {
            bail!("services.events_transport must use NATS for messaging gateway");
        }
        Some(ServiceTransportConfig::Noop) => Ok(None),
        None => Ok(None),
    }
}

fn install_store_path(config: &GreenticConfig) -> Option<PathBuf> {
    let root = &config.paths.greentic_root;
    if let Some(path) = install_store_path_from_file(root.join(".greentic/install_store.path")) {
        return Some(path);
    }
    if let Some(path) = install_store_path_from_file(root.join(".greentic/dev/install_store.path"))
    {
        return Some(path);
    }
    let path = root.join(".greentic/dev/installs.json");
    path.exists().then_some(path)
}

fn install_store_path_from_file(path: PathBuf) -> Option<PathBuf> {
    let raw = fs::read_to_string(&path).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let candidate = PathBuf::from(trimmed);
    Some(candidate)
}
