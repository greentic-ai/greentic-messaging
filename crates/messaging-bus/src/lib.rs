use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::Mutex;

pub const INGRESS_SUBJECT_PREFIX: &str = "greentic.messaging.ingress";
pub const EGRESS_SUBJECT_PREFIX: &str = "greentic.messaging.egress.out";

#[derive(thiserror::Error, Debug)]
pub enum BusError {
    #[error(transparent)]
    Publish(#[from] anyhow::Error),
}

#[async_trait]
pub trait BusClient: Send + Sync {
    async fn publish_value(&self, subject: &str, payload: Value) -> Result<(), BusError>;
}

pub struct NatsBusClient {
    client: async_nats::Client,
}

impl NatsBusClient {
    pub fn new(client: async_nats::Client) -> Self {
        Self { client }
    }
}

#[async_trait]
impl BusClient for NatsBusClient {
    async fn publish_value(&self, subject: &str, payload: Value) -> Result<(), BusError> {
        let bytes =
            serde_json::to_vec(&payload).map_err(|e| BusError::Publish(anyhow::Error::new(e)))?;
        self.client
            .publish(subject.to_string(), bytes.into())
            .await
            .map_err(|err| BusError::Publish(anyhow::Error::new(err)))
    }
}

#[derive(Clone, Default)]
pub struct InMemoryBusClient {
    published: Arc<Mutex<Vec<(String, Value)>>>,
}

impl InMemoryBusClient {
    pub async fn take_published(&self) -> Vec<(String, Value)> {
        let mut guard = self.published.lock().await;
        std::mem::take(&mut *guard)
    }
}

#[async_trait]
impl BusClient for InMemoryBusClient {
    async fn publish_value(&self, subject: &str, payload: Value) -> Result<(), BusError> {
        let mut guard = self.published.lock().await;
        guard.push((subject.to_string(), payload));
        Ok(())
    }
}

pub fn to_value<T: serde::Serialize>(payload: &T) -> Result<Value, BusError> {
    serde_json::to_value(payload).map_err(|e| BusError::Publish(anyhow::Error::new(e)))
}

/// Build the canonical ingress subject.
pub fn ingress_subject(env: &str, tenant: &str, team: &str, platform: &str) -> String {
    ingress_subject_with_prefix(INGRESS_SUBJECT_PREFIX, env, tenant, team, platform)
}

/// Build the canonical ingress subject using a custom prefix.
pub fn ingress_subject_with_prefix(
    prefix: &str,
    env: &str,
    tenant: &str,
    team: &str,
    platform: &str,
) -> String {
    format!("{prefix}.{env}.{tenant}.{team}.{platform}")
}

/// Build the canonical egress subject.
pub fn egress_subject(tenant: &str, platform: &str) -> String {
    egress_subject_with_prefix(EGRESS_SUBJECT_PREFIX, tenant, platform)
}

/// Build the canonical egress subject using a custom prefix.
pub fn egress_subject_with_prefix(prefix: &str, tenant: &str, platform: &str) -> String {
    format!("{prefix}.{tenant}.{platform}")
}
