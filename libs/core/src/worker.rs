use crate::{ChannelMessage, OutboundEnvelope, TenantCtx};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::time::Duration;
use thiserror::Error;
use time::OffsetDateTime;
use tracing::{error, warn};

/// Worker envelope schema version.
pub const WORKER_ENVELOPE_VERSION: &str = "1.0";
/// Canonical default worker identifier for the repo assistant.
pub const DEFAULT_WORKER_ID: &str = "greentic-repo-assistant";
/// Default NATS subject used when no override is provided.
pub const DEFAULT_WORKER_NATS_SUBJECT: &str = "workers.repo-assistant";

/// Which transport to use for talking to the worker endpoint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkerTransport {
    Nats,
    Http,
}

impl WorkerTransport {
    pub fn from_env(value: Option<String>) -> Self {
        match value
            .unwrap_or_else(|| "nats".to_string())
            .to_ascii_lowercase()
            .as_str()
        {
            "http" => WorkerTransport::Http,
            _ => WorkerTransport::Nats,
        }
    }
}

/// Routing configuration for the repo worker.
#[derive(Clone, Debug)]
pub struct WorkerRoutingConfig {
    pub transport: WorkerTransport,
    pub worker_id: String,
    pub nats_subject: String,
    pub http_url: Option<String>,
    /// How many transient retries to attempt locally before surfacing an error.
    pub max_retries: u8,
}

impl Default for WorkerRoutingConfig {
    fn default() -> Self {
        Self {
            transport: WorkerTransport::Nats,
            worker_id: DEFAULT_WORKER_ID.to_string(),
            nats_subject: DEFAULT_WORKER_NATS_SUBJECT.to_string(),
            http_url: None,
            max_retries: 2,
        }
    }
}

impl WorkerRoutingConfig {
    pub fn from_env() -> Self {
        let transport = WorkerTransport::from_env(std::env::var("REPO_WORKER_TRANSPORT").ok());
        let worker_id = std::env::var("REPO_WORKER_ID")
            .ok()
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| DEFAULT_WORKER_ID.to_string());
        let nats_subject = std::env::var("REPO_WORKER_NATS_SUBJECT")
            .ok()
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| DEFAULT_WORKER_NATS_SUBJECT.to_string());
        let http_url = std::env::var("REPO_WORKER_HTTP_URL").ok();
        let max_retries = std::env::var("REPO_WORKER_RETRIES")
            .ok()
            .and_then(|v| v.parse::<u8>().ok())
            .unwrap_or(2);

        Self {
            transport,
            worker_id,
            nats_subject,
            http_url,
            max_retries,
        }
    }
}

/// Inbound request sent to the repo worker.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkerRequest {
    pub version: String,
    pub tenant: TenantCtx,
    pub worker_id: String,
    #[serde(default)]
    pub metadata: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    pub payload: Value,
    pub timestamp_utc: String,
}

impl WorkerRequest {
    pub fn new(
        tenant: TenantCtx,
        worker_id: String,
        payload: Value,
        session_id: Option<String>,
        thread_id: Option<String>,
        correlation_id: Option<String>,
        metadata: BTreeMap<String, Value>,
    ) -> Self {
        Self {
            version: WORKER_ENVELOPE_VERSION.to_string(),
            tenant,
            worker_id,
            metadata,
            correlation_id,
            session_id,
            thread_id,
            payload,
            timestamp_utc: OffsetDateTime::now_utc()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_else(|_| OffsetDateTime::now_utc().unix_timestamp().to_string()),
        }
    }

    pub fn from_channel(
        channel: &ChannelMessage,
        payload: Value,
        config: &WorkerRoutingConfig,
        correlation_id: Option<String>,
    ) -> Self {
        let mut metadata = BTreeMap::new();
        metadata.insert(
            "channel_id".into(),
            Value::String(channel.channel_id.clone()),
        );
        if let Some(route) = &channel.route {
            metadata.insert("route".into(), Value::String(route.clone()));
        }
        Self::new(
            channel.tenant.clone(),
            config.worker_id.clone(),
            payload,
            Some(channel.session_id.clone()),
            None,
            correlation_id,
            metadata,
        )
    }
}

/// A single worker-generated message (text/card/event payload).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkerMessage {
    pub kind: String,
    pub payload: Value,
}

/// Response from the repo worker; may carry multiple messages.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkerResponse {
    pub version: String,
    pub tenant: TenantCtx,
    pub worker_id: String,
    #[serde(default)]
    pub metadata: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(default)]
    pub messages: Vec<WorkerMessage>,
    pub timestamp_utc: String,
}

impl WorkerResponse {
    pub fn empty_for(request: &WorkerRequest) -> Self {
        Self {
            version: request.version.clone(),
            tenant: request.tenant.clone(),
            worker_id: request.worker_id.clone(),
            metadata: request.metadata.clone(),
            correlation_id: request.correlation_id.clone(),
            session_id: request.session_id.clone(),
            thread_id: request.thread_id.clone(),
            messages: Vec::new(),
            timestamp_utc: OffsetDateTime::now_utc()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_else(|_| OffsetDateTime::now_utc().unix_timestamp().to_string()),
        }
    }
}

/// Converts a worker response into outbound envelopes targeting the same channel context.
pub fn worker_messages_to_outbound(
    response: &WorkerResponse,
    channel: &ChannelMessage,
) -> Vec<OutboundEnvelope> {
    response
        .messages
        .iter()
        .map(|msg| {
            let mut meta = serde_json::Map::new();
            meta.insert(
                "worker_id".into(),
                Value::String(response.worker_id.clone()),
            );
            if let Some(corr) = &response.correlation_id {
                meta.insert("correlation_id".into(), Value::String(corr.clone()));
            }
            meta.insert("kind".into(), Value::String(msg.kind.clone()));
            for (k, v) in &response.metadata {
                meta.insert(k.clone(), v.clone());
            }

            OutboundEnvelope {
                tenant: channel.tenant.clone(),
                channel_id: channel.channel_id.clone(),
                session_id: channel.session_id.clone(),
                meta: Value::Object(meta),
                body: msg.payload.clone(),
            }
        })
        .collect()
}

#[derive(Debug, Error)]
pub enum WorkerClientError {
    #[error("failed to serialize worker request: {0}")]
    Serialize(#[source] serde_json::Error),
    #[error("failed to deserialize worker response: {0}")]
    Deserialize(#[source] serde_json::Error),
    #[error("NATS request failed: {0}")]
    Nats(#[source] anyhow::Error),
    #[error("HTTP request failed: {0}")]
    Http(#[source] anyhow::Error),
}

#[async_trait]
pub trait WorkerClient: Send + Sync {
    async fn send_request(
        &self,
        request: WorkerRequest,
    ) -> Result<WorkerResponse, WorkerClientError>;
}

/// In-memory client used in tests.
pub struct InMemoryWorkerClient {
    responder: Box<dyn Fn(WorkerRequest) -> WorkerResponse + Send + Sync>,
}

impl InMemoryWorkerClient {
    pub fn new<F>(responder: F) -> Self
    where
        F: Fn(WorkerRequest) -> WorkerResponse + Send + Sync + 'static,
    {
        Self {
            responder: Box::new(responder),
        }
    }
}

#[async_trait]
impl WorkerClient for InMemoryWorkerClient {
    async fn send_request(
        &self,
        request: WorkerRequest,
    ) -> Result<WorkerResponse, WorkerClientError> {
        Ok((self.responder)(request))
    }
}

/// Sends a worker request via the provided client and maps the response back to outbound envelopes.
pub async fn forward_to_worker(
    client: &dyn WorkerClient,
    channel: &ChannelMessage,
    payload: Value,
    config: &WorkerRoutingConfig,
    correlation_id: Option<String>,
) -> Result<Vec<OutboundEnvelope>, WorkerClientError> {
    let request = WorkerRequest::from_channel(channel, payload, config, correlation_id);
    let response = client.send_request(request).await?;
    Ok(worker_messages_to_outbound(&response, channel))
}

#[cfg(feature = "nats")]
pub struct NatsWorkerClient {
    client: async_nats::Client,
    subject: String,
    max_retries: u8,
}

#[cfg(feature = "nats")]
impl NatsWorkerClient {
    pub fn new(client: async_nats::Client, subject: String, max_retries: u8) -> Self {
        Self {
            client,
            subject,
            max_retries,
        }
    }

    async fn send_once(
        &self,
        request: &WorkerRequest,
    ) -> Result<WorkerResponse, WorkerClientError> {
        let bytes = serde_json::to_vec(request).map_err(WorkerClientError::Serialize)?;
        let msg = self
            .client
            .request(self.subject.clone(), bytes.into())
            .await
            .map_err(|e| WorkerClientError::Nats(anyhow::Error::new(e)))?;
        serde_json::from_slice(&msg.payload).map_err(WorkerClientError::Deserialize)
    }
}

#[cfg(feature = "nats")]
#[async_trait]
impl WorkerClient for NatsWorkerClient {
    async fn send_request(
        &self,
        request: WorkerRequest,
    ) -> Result<WorkerResponse, WorkerClientError> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            match self.send_once(&request).await {
                Ok(res) => return Ok(res),
                Err(err) => {
                    if attempt > self.max_retries {
                        return Err(err);
                    }
                    warn!(attempt, subject = %self.subject, error = %err, "retrying worker request over NATS");
                    tokio::time::sleep(Duration::from_millis(50 * attempt as u64)).await;
                }
            }
        }
    }
}

pub struct HttpWorkerClient {
    client: reqwest::Client,
    url: String,
    max_retries: u8,
}

impl HttpWorkerClient {
    pub fn new(url: String, max_retries: u8) -> Self {
        Self {
            client: reqwest::Client::new(),
            url,
            max_retries,
        }
    }

    async fn send_once(
        &self,
        request: &WorkerRequest,
    ) -> Result<WorkerResponse, WorkerClientError> {
        let response = self
            .client
            .post(&self.url)
            .json(request)
            .send()
            .await
            .map_err(|e| WorkerClientError::Http(anyhow::Error::new(e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(WorkerClientError::Http(anyhow::anyhow!(
                "HTTP {} from worker endpoint: {}",
                status,
                body
            )));
        }

        let body = response
            .bytes()
            .await
            .map_err(|e| WorkerClientError::Http(anyhow::Error::new(e)))?;
        serde_json::from_slice(&body).map_err(WorkerClientError::Deserialize)
    }
}

#[async_trait]
impl WorkerClient for HttpWorkerClient {
    async fn send_request(
        &self,
        request: WorkerRequest,
    ) -> Result<WorkerResponse, WorkerClientError> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            match self.send_once(&request).await {
                Ok(res) => return Ok(res),
                Err(err) => {
                    if attempt > self.max_retries {
                        error!(attempt, url = %self.url, error = %err, "worker HTTP request failed");
                        return Err(err);
                    }
                    warn!(attempt, url = %self.url, error = %err, "retrying worker HTTP request");
                    tokio::time::sleep(Duration::from_millis(50 * attempt as u64)).await;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_channel() -> ChannelMessage {
        ChannelMessage {
            tenant: crate::make_tenant_ctx("acme".into(), Some("team".into()), None),
            channel_id: "webchat".into(),
            session_id: "sess-1".into(),
            route: None,
            payload: serde_json::json!({"text": "hi"}),
        }
    }

    #[tokio::test]
    async fn builds_request_and_maps_response() {
        let channel = sample_channel();
        let config = WorkerRoutingConfig::default();
        let payload = serde_json::json!({"body": "hello"});
        let corr = Some("corr-1".to_string());
        let client = InMemoryWorkerClient::new(|req| {
            assert_eq!(req.version, WORKER_ENVELOPE_VERSION);
            assert_eq!(req.worker_id, DEFAULT_WORKER_ID);
            assert_eq!(req.session_id.as_deref(), Some("sess-1"));
            assert_eq!(req.correlation_id.as_deref(), Some("corr-1"));
            let mut resp = WorkerResponse::empty_for(&req);
            resp.metadata = req.metadata.clone();
            resp.messages = vec![WorkerMessage {
                kind: "text".into(),
                payload: serde_json::json!({"reply": "pong"}),
            }];
            resp
        });

        let outbound = forward_to_worker(&client, &channel, payload, &config, corr)
            .await
            .unwrap();

        assert_eq!(outbound.len(), 1);
        assert_eq!(outbound[0].channel_id, "webchat");
        assert_eq!(outbound[0].body["reply"], "pong");
        assert_eq!(outbound[0].tenant.tenant.as_str(), "acme");
        assert_eq!(outbound[0].session_id, "sess-1");
        assert_eq!(outbound[0].meta["kind"], "text");
        assert_eq!(outbound[0].meta["worker_id"], DEFAULT_WORKER_ID);
        assert_eq!(outbound[0].meta["correlation_id"], "corr-1");
    }
}
