use anyhow::Result;
use async_trait::async_trait;
use reqwest::Client;
use serde::Serialize;
use std::sync::Arc;
use tracing::info;

use crate::{AdapterDescriptor, OutMessage};

/// Abstraction for invoking adapter components via an external runner.
///
/// Implementations can use HTTP, NATS, or any other transport; this crate does not
/// assume a concrete runner dependency.
#[async_trait]
pub trait RunnerClient: Send + Sync {
    async fn invoke_adapter(&self, out: &OutMessage, adapter: &AdapterDescriptor) -> Result<()>;
}

/// Default stub client that only logs invocation. Useful for local/dev and tests.
#[derive(Default)]
pub struct LoggingRunnerClient;

#[async_trait]
impl RunnerClient for LoggingRunnerClient {
    async fn invoke_adapter(&self, out: &OutMessage, adapter: &AdapterDescriptor) -> Result<()> {
        info!(
            tenant = %out.tenant,
            platform = %out.platform.as_str(),
            adapter = %adapter.name,
            component = %adapter.component,
            flow = ?adapter.flow_path(),
            "RunnerClient stub invoked adapter"
        );
        Ok(())
    }
}

/// Helper to wrap a shared client.
pub fn shared_client<C: RunnerClient + 'static>(client: C) -> Arc<C> {
    Arc::new(client)
}

/// Simple HTTP-based runner client that POSTs adapter invocations to an external runner service.
#[derive(Clone)]
pub struct HttpRunnerClient {
    client: Client,
    url: String,
    api_key: Option<String>,
}

impl HttpRunnerClient {
    pub fn new(url: impl Into<String>, api_key: Option<String>) -> Result<Self> {
        Ok(Self {
            client: Client::new(),
            url: url.into(),
            api_key,
        })
    }
}

#[derive(Serialize)]
struct InvocationPayload<'a> {
    adapter: &'a AdapterDescriptor,
    message: &'a OutMessage,
}

#[async_trait]
impl RunnerClient for HttpRunnerClient {
    async fn invoke_adapter(&self, out: &OutMessage, adapter: &AdapterDescriptor) -> Result<()> {
        let payload = InvocationPayload {
            adapter,
            message: out,
        };
        let mut req = self.client.post(&self.url).json(&payload);
        if let Some(key) = &self.api_key {
            req = req.header("Authorization", format!("Bearer {key}"));
        }
        let resp = req.send().await?;
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            anyhow::bail!("runner returned {} body={}", status, body);
        }
        Ok(())
    }
}
