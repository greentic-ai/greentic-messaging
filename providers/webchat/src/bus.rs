use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use tracing::debug;

use crate::types::GreenticEvent;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Subject(String);

impl Subject {
    pub fn incoming(env: &str, tenant: &str, team: Option<&str>) -> Self {
        let team_segment = team.filter(|s| !s.is_empty()).unwrap_or("-");
        Self(format!(
            "greentic.{env}.{tenant}.{team}.events.incoming",
            env = env.to_lowercase(),
            tenant = tenant.to_lowercase(),
            team = team_segment.to_lowercase()
        ))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[async_trait]
pub trait EventBus: Send + Sync {
    async fn publish(&self, subject: &Subject, event: &GreenticEvent) -> Result<()>;
}

#[derive(Clone, Default)]
pub struct NoopBus;

#[async_trait]
impl EventBus for NoopBus {
    async fn publish(&self, subject: &Subject, _event: &GreenticEvent) -> Result<()> {
        debug!(
            target = "webchat.bus",
            subject = subject.as_str(),
            "dropping event (noop bus)"
        );
        Ok(())
    }
}

#[cfg(feature = "nats")]
#[derive(Clone)]
pub struct NatsBus {
    client: async_nats::Client,
}

#[cfg(feature = "nats")]
impl NatsBus {
    pub fn new(client: async_nats::Client) -> Self {
        Self { client }
    }
}

#[cfg(feature = "nats")]
#[async_trait]
impl EventBus for NatsBus {
    async fn publish(&self, subject: &Subject, event: &GreenticEvent) -> Result<()> {
        let payload = serde_json::to_vec(event)?;
        self.client
            .publish(subject.as_str(), payload.into())
            .await?;
        Ok(())
    }
}

pub type SharedBus = Arc<dyn EventBus>;
