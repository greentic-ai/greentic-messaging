use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::Mutex;

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
