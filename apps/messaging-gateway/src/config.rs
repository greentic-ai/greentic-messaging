use std::net::{IpAddr, SocketAddr};
use std::str::FromStr;

use anyhow::{Context, Result, bail};
use greentic_types::EnvId;
use gsm_core::WorkerTransport;

#[derive(Debug, Clone)]
pub struct GatewayConfig {
    pub env: EnvId,
    pub nats_url: String,
    pub addr: SocketAddr,
    pub default_team: String,
    pub subject_prefix: String,
    pub worker_routing: Option<gsm_core::WorkerRoutingConfig>,
    pub worker_routes: std::collections::BTreeMap<String, gsm_core::WorkerRoutingConfig>,
    pub worker_egress_subject: Option<String>,
}

impl GatewayConfig {
    pub fn from_env() -> Result<Self> {
        let env = EnvId(std::env::var("GREENTIC_ENV").unwrap_or_else(|_| "dev".into()));
        let nats_url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
        let default_addr =
            std::env::var("MESSAGING_GATEWAY_ADDR").unwrap_or_else(|_| "0.0.0.0".into());
        let port = std::env::var("MESSAGING_GATEWAY_PORT")
            .ok()
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(8080);
        let ip = IpAddr::from_str(&default_addr).context("invalid MESSAGING_GATEWAY_ADDR")?;
        let worker_routes = gsm_core::worker_routes_from_env();
        let worker_routing = {
            let enabled = std::env::var("REPO_WORKER_ENABLE")
                .ok()
                .map(|v| v.eq_ignore_ascii_case("true"))
                .unwrap_or(false);
            if enabled {
                let cfg = gsm_core::WorkerRoutingConfig::from_env();
                if cfg.transport == WorkerTransport::Http
                    && cfg.http_url.as_deref().map(str::is_empty).unwrap_or(true)
                {
                    bail!("REPO_WORKER_HTTP_URL must be set when REPO_WORKER_TRANSPORT=http");
                }
                Some(cfg)
            } else {
                None
            }
        };
        Ok(Self {
            env: env.clone(),
            nats_url,
            addr: SocketAddr::new(ip, port),
            default_team: std::env::var("MESSAGING_GATEWAY_DEFAULT_TEAM")
                .unwrap_or_else(|_| "default".into()),
            subject_prefix: std::env::var("MESSAGING_INGRESS_SUBJECT_PREFIX")
                .unwrap_or_else(|_| gsm_bus::INGRESS_SUBJECT_PREFIX.to_string()),
            worker_routing,
            worker_routes,
            worker_egress_subject: {
                let base = std::env::var("MESSAGING_EGRESS_SUBJECT")
                    .unwrap_or_else(|_| format!("greentic.messaging.egress.{}", env.0.clone()));
                let trimmed = base.trim_end_matches('>').trim_end_matches('.');
                Some(format!("{trimmed}.repo-worker"))
            },
        })
    }
}
