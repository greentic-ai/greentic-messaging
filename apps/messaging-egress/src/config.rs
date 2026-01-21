use anyhow::{Result, bail};
use greentic_config::ConfigResolver;
use greentic_config_types::{GreenticConfig, ServiceTransportConfig};
use greentic_types::EnvId;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct EgressConfig {
    pub env: EnvId,
    pub nats_url: String,
    pub subject_filter: String,
    pub adapter: Option<String>,
    pub packs_root: String,
    pub egress_prefix: String,
    pub runner_http_url: Option<String>,
    pub runner_http_api_key: Option<String>,
    pub install_store_path: Option<PathBuf>,
}

impl EgressConfig {
    pub fn load() -> Result<Self> {
        let resolved = ConfigResolver::new().load()?;
        Self::from_config(&resolved.config)
    }

    pub fn from_config(config: &GreenticConfig) -> Result<Self> {
        let env = config.environment.env_id.clone();
        let nats_url =
            nats_url_from_config(config)?.unwrap_or_else(|| "nats://127.0.0.1:4222".to_string());
        let egress_prefix = egress_prefix_from_config(config)
            .unwrap_or_else(|| gsm_core::EGRESS_SUBJECT_PREFIX.to_string());
        let base = format!("{}.{}", egress_prefix, env.0);

        let subject_filter = if base.contains('>') {
            base
        } else if base.ends_with('.') {
            format!("{base}>")
        } else {
            format!("{base}.>")
        };

        Ok(Self {
            env,
            nats_url,
            subject_filter,
            adapter: None,
            packs_root: packs_root_from_config(config)
                .unwrap_or_else(|| PathBuf::from("packs"))
                .to_string_lossy()
                .to_string(),
            egress_prefix,
            runner_http_url: runner_http_url_from_config(config),
            runner_http_api_key: None,
            install_store_path: install_store_path(config),
        })
    }
}

fn packs_root_from_config(config: &GreenticConfig) -> Option<PathBuf> {
    Some(config.paths.greentic_root.join("packs"))
}

fn nats_url_from_config(config: &GreenticConfig) -> Result<Option<String>> {
    let transport = config
        .services
        .as_ref()
        .and_then(|services| services.events_transport.as_ref())
        .and_then(|svc| svc.transport.as_ref());
    match transport {
        Some(ServiceTransportConfig::Nats { url, .. }) => Ok(Some(url.to_string())),
        Some(ServiceTransportConfig::Http { .. }) => {
            bail!("services.events_transport must use NATS for messaging egress");
        }
        Some(ServiceTransportConfig::Noop) => Ok(None),
        None => Ok(None),
    }
}

fn egress_prefix_from_config(config: &GreenticConfig) -> Option<String> {
    let transport = config
        .services
        .as_ref()
        .and_then(|services| services.publish.as_ref())
        .and_then(|svc| svc.transport.as_ref());
    match transport {
        Some(ServiceTransportConfig::Nats { subject_prefix, .. }) => subject_prefix.clone(),
        _ => None,
    }
}

fn runner_http_url_from_config(config: &GreenticConfig) -> Option<String> {
    let transport = config
        .services
        .as_ref()
        .and_then(|services| services.runner.as_ref())
        .and_then(|svc| svc.transport.as_ref());
    match transport {
        Some(ServiceTransportConfig::Http { url, .. }) => Some(url.to_string()),
        _ => None,
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
    Some(PathBuf::from(trimmed))
}
