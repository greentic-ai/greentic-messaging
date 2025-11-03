use std::sync::Arc;

use anyhow::anyhow;
use async_trait::async_trait;
use gsm_core::TenantCtx;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::errors::MsgError;
use crate::manifest::ProviderManifest;
use crate::registry::{ProviderBuilder, ProviderRegistry};
use crate::traits::{Message, ReceiveAdapter, SendAdapter, SendResult};

const MANIFEST_STR: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../providers/webchat/provider.json"
));

pub fn register(registry: &mut ProviderRegistry) -> Result<(), anyhow::Error> {
    let manifest = ProviderManifest::from_json(MANIFEST_STR)?;
    let manifest_for_send = Arc::new(manifest.clone());
    let builder = ProviderBuilder::new()
        .with_send(move || {
            let manifest = Arc::clone(&manifest_for_send);
            WebChatSendAdapter::from_manifest(&manifest)
                .map(|adapter| Box::new(adapter) as Box<dyn SendAdapter>)
        })
        .with_receive(|| Ok(Box::new(WebChatReceiveAdapter) as Box<dyn ReceiveAdapter>));

    registry
        .register(manifest, builder)
        .map_err(|err| anyhow!(err))
}

struct WebChatSendAdapter {
    client: reqwest::Client,
    default_endpoint: String,
}

impl WebChatSendAdapter {
    fn from_manifest(manifest: &ProviderManifest) -> Result<Self, MsgError> {
        let endpoint = manifest.endpoints.send.clone();
        let client = reqwest::Client::builder()
            .user_agent("gsm-webchat-provider/0.1")
            .build()
            .map_err(|err| {
                MsgError::retryable("webchat_client", "failed to create HTTP client", None)
                    .with_source(err)
            })?;
        Ok(Self {
            client,
            default_endpoint: endpoint,
        })
    }

    fn endpoint(&self) -> String {
        std::env::var("WEBCHAT_SEND_URL").unwrap_or_else(|_| self.default_endpoint.clone())
    }
}

#[async_trait]
impl SendAdapter for WebChatSendAdapter {
    async fn send(&self, _ctx: &TenantCtx, message: &Message) -> Result<SendResult, MsgError> {
        let to = message
            .to
            .clone()
            .or_else(|| message.chat_id.clone())
            .ok_or_else(|| {
                MsgError::permanent("webchat_missing_to", "missing message recipient")
            })?;
        let payload = json!({
            "to": to,
            "text": message.text.clone().unwrap_or_default(),
            "thread_id": message.thread_id.clone(),
        });

        let endpoint = self.endpoint();
        if let Some(scenario) = endpoint.strip_prefix("mock://") {
            return match scenario {
                "success" => Ok(SendResult {
                    provider_message_id: format!("mock:{}", to),
                    delivered: true,
                    raw: payload.clone(),
                }),
                "throttle" => Err(MsgError::retryable(
                    "webchat_retryable",
                    "mock throttled",
                    Some(1_000),
                )),
                other => Err(MsgError::permanent(
                    "webchat_mock",
                    format!("unknown mock scenario `{other}`"),
                )),
            };
        }

        let response = self
            .client
            .post(&endpoint)
            .json(&payload)
            .send()
            .await
            .map_err(|err| {
                MsgError::retryable(
                    "webchat_http",
                    "failed to call webchat endpoint",
                    Some(1_000),
                )
                .with_source(err)
            })?;

        let status = response.status();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|value| value.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
            .map(|seconds| seconds * 1_000);
        let body_text = response.text().await.map_err(|err| {
            MsgError::retryable("webchat_body", "failed to read response body", Some(1_000))
                .with_source(err)
        })?;
        let raw: Value = serde_json::from_str(&body_text).unwrap_or(Value::Null);

        if !status.is_success() {
            let retryable = status.as_u16() == 429 || status.is_server_error();
            let code = if retryable {
                "webchat_retryable"
            } else {
                "webchat_send_failed"
            };
            let backoff = if status.as_u16() == 429 {
                retry_after
            } else {
                None
            };
            let err = if retryable {
                MsgError::retryable(
                    code,
                    format!("status={} body={}", status.as_u16(), body_text),
                    backoff,
                )
            } else {
                MsgError::permanent(
                    code,
                    format!("status={} body={}", status.as_u16(), body_text),
                )
            };
            return Err(err.with_source(anyhow!("webchat send failed")));
        }

        let response: WebChatSendResponse = serde_json::from_value(raw.clone()).map_err(|err| {
            MsgError::permanent("webchat_response", "response missing required fields")
                .with_source(err)
        })?;

        Ok(SendResult {
            provider_message_id: response.id,
            delivered: response.delivered,
            raw,
        })
    }
}

#[derive(Deserialize)]
struct WebChatSendResponse {
    id: String,
    #[serde(default)]
    delivered: bool,
}

#[derive(Default)]
struct WebChatReceiveAdapter;

#[derive(Debug, Deserialize, Serialize)]
struct IncomingBatch {
    #[serde(default)]
    messages: Vec<IncomingMessage>,
}

#[derive(Debug, Deserialize, Serialize)]
struct IncomingMessage {
    #[serde(rename = "chatId")]
    chat_id: String,
    #[serde(rename = "userId")]
    user_id: String,
    text: String,
    #[serde(rename = "threadId")]
    thread_id: Option<String>,
}

impl ReceiveAdapter for WebChatReceiveAdapter {
    fn ingest(&self, _ctx: &TenantCtx, payload: Value) -> Result<Vec<Message>, MsgError> {
        let batch: IncomingBatch = serde_json::from_value(payload).map_err(|err| {
            MsgError::permanent("webchat_payload", "invalid webhook payload").with_source(err)
        })?;
        let mut out = Vec::with_capacity(batch.messages.len());
        for item in batch.messages {
            let metadata = serde_json::to_value(&item).unwrap_or(Value::Null);
            out.push(Message {
                chat_id: Some(item.chat_id.clone()),
                user_id: Some(item.user_id.clone()),
                text: Some(item.text.clone()),
                thread_id: item.thread_id.clone(),
                metadata,
                ..Default::default()
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gsm_core::make_tenant_ctx;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn restore_env(key: &str, previous: Option<String>) {
        if let Some(value) = previous {
            unsafe {
                std::env::set_var(key, value);
            }
        } else {
            unsafe {
                std::env::remove_var(key);
            }
        }
    }

    #[tokio::test]
    async fn send_succeeds() {
        let _guard = env_lock().lock().unwrap();
        let manifest = ProviderManifest::from_json(MANIFEST_STR).unwrap();
        let mut adapter = WebChatSendAdapter::from_manifest(&manifest).unwrap();
        adapter.default_endpoint = "mock://success".into();
        let prev = std::env::var("WEBCHAT_SEND_URL").ok();
        unsafe {
            std::env::remove_var("WEBCHAT_SEND_URL");
        }

        let ctx = make_tenant_ctx("acme".into(), None, None);
        let message = Message {
            to: Some("user-1".into()),
            text: Some("hello".into()),
            ..Default::default()
        };

        let result = adapter.send(&ctx, &message).await.unwrap();
        assert_eq!(result.provider_message_id, "mock:user-1");
        assert!(result.delivered);
        assert_eq!(
            result.raw.get("to").and_then(|v| v.as_str()),
            Some("user-1")
        );

        restore_env("WEBCHAT_SEND_URL", prev);
    }

    #[tokio::test]
    async fn send_handles_429() {
        let _guard = env_lock().lock().unwrap();
        let manifest = ProviderManifest::from_json(MANIFEST_STR).unwrap();
        let adapter = WebChatSendAdapter::from_manifest(&manifest).unwrap();
        let prev = std::env::var("WEBCHAT_SEND_URL").ok();
        unsafe {
            std::env::set_var("WEBCHAT_SEND_URL", "mock://throttle");
        }

        let ctx = make_tenant_ctx("acme".into(), None, None);
        let message = Message {
            to: Some("user-1".into()),
            text: Some("hello".into()),
            ..Default::default()
        };

        let err = adapter.send(&ctx, &message).await.err().unwrap();
        assert!(err.is_retryable());

        restore_env("WEBCHAT_SEND_URL", prev);
    }

    #[test]
    fn receive_parses_messages() {
        let adapter = WebChatReceiveAdapter::default();
        let ctx = make_tenant_ctx("acme".into(), None, None);
        let payload = json!({
            "messages": [
                { "chatId": "room-1", "userId": "user-1", "text": "hi" }
            ]
        });

        let messages = adapter.ingest(&ctx, payload).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].chat_id.as_deref(), Some("room-1"));
        assert_eq!(messages[0].user_id.as_deref(), Some("user-1"));
    }
}
