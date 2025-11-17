use std::net::{IpAddr, SocketAddr};
use std::str::FromStr;

use anyhow::{Context, Result};
use greentic_types::EnvId;

#[derive(Debug, Clone)]
pub struct GatewayConfig {
    pub env: EnvId,
    pub nats_url: String,
    pub addr: SocketAddr,
    pub default_team: String,
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
        Ok(Self {
            env,
            nats_url,
            addr: SocketAddr::new(ip, port),
            default_team: std::env::var("MESSAGING_GATEWAY_DEFAULT_TEAM")
                .unwrap_or_else(|_| "default".into()),
        })
    }
}
