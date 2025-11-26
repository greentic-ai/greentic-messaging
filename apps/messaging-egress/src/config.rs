use anyhow::Result;
use greentic_types::EnvId;

#[derive(Debug, Clone)]
pub struct EgressConfig {
    pub env: EnvId,
    pub nats_url: String,
    pub subject_filter: String,
    pub adapter: Option<String>,
    pub packs_root: String,
    pub egress_prefix: String,
}

impl EgressConfig {
    pub fn from_env() -> Result<Self> {
        let env = EnvId(std::env::var("GREENTIC_ENV").unwrap_or_else(|_| "dev".into()));
        let nats_url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
        let egress_prefix = std::env::var("MESSAGING_EGRESS_OUT_PREFIX")
            .unwrap_or_else(|_| messaging_bus::EGRESS_SUBJECT_PREFIX.to_string());
        let base = std::env::var("MESSAGING_EGRESS_SUBJECT")
            .unwrap_or_else(|_| format!("greentic.messaging.egress.{}", env.0));

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
            adapter: std::env::var("MESSAGING_EGRESS_ADAPTER").ok(),
            packs_root: std::env::var("MESSAGING_PACKS_ROOT").unwrap_or_else(|_| "packs".into()),
            egress_prefix,
        })
    }
}
