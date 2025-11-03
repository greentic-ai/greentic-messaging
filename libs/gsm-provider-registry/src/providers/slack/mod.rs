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
    "/../../providers/slack/provider.json"
));

pub fn register(registry: &mut ProviderRegistry) -> Result<(), anyhow::Error> {
    let manifest = ProviderManifest::from_json(MANIFEST_STR)?;
    let manifest_for_send = Arc::new(manifest.clone());
    let builder = ProviderBuilder::new()
        .with_send(move || {
            let manifest = Arc::clone(&manifest_for_send);
            SlackSendAdapter::from_manifest(&manifest)
                .map(|adapter| Box::new(adapter) as Box<dyn SendAdapter>)
        })
        .with_receive(|| Ok(Box::new(SlackReceiveAdapter) as Box<dyn ReceiveAdapter>));

    registry
        .register(manifest, builder)
        .map_err(|err| anyhow!(err))
}

struct SlackSendAdapter {
    client: reqwest::Client,
    endpoint: String,
    default_token: Option<String>,
}

impl SlackSendAdapter {
    fn from_manifest(manifest: &ProviderManifest) -> Result<Self, MsgError> {
        let client = reqwest::Client::builder()
            .user_agent("gsm-slack-provider/0.1")
            .build()
            .map_err(|err| {
                MsgError::permanent("slack_client", "failed to create HTTP client").with_source(err)
            })?;
        Ok(Self {
            client,
            endpoint: manifest.endpoints.send.clone(),
            default_token: std::env::var("SLACK_BOT_TOKEN").ok(),
        })
    }

    fn resolve_token(&self) -> Result<String, MsgError> {
        if let Ok(value) = std::env::var("SLACK_BOT_TOKEN")
            && !value.trim().is_empty()
        {
            return Ok(value);
        }
        if let Some(value) = self.default_token.as_ref()
            && !value.trim().is_empty()
        {
            return Ok(value.clone());
        }
        Err(MsgError::permanent(
            "slack_missing_token",
            "SLACK_BOT_TOKEN is not configured",
        ))
    }

    fn endpoint(&self) -> String {
        std::env::var("SLACK_SEND_URL").unwrap_or_else(|_| self.endpoint.clone())
    }

    fn build_payload(&self, message: &Message) -> Result<Value, MsgError> {
        let channel = message
            .chat_id
            .clone()
            .or_else(|| message.to.clone())
            .ok_or_else(|| {
                MsgError::permanent("slack_missing_channel", "channel or recipient required")
            })?;

        let mut payload = json!({
            "channel": channel,
            "text": message.text.clone().unwrap_or_default(),
        });

        if let Some(thread) = message.thread_id.clone() {
            payload
                .as_object_mut()
                .expect("payload object")
                .insert("thread_ts".into(), Value::String(thread));
        }

        Ok(payload)
    }
}

#[async_trait]
impl SendAdapter for SlackSendAdapter {
    async fn send(&self, _ctx: &TenantCtx, message: &Message) -> Result<SendResult, MsgError> {
        let token = self.resolve_token()?;
        let payload = self.build_payload(message)?;
        let endpoint = self.endpoint();

        if let Some(scenario) = endpoint.strip_prefix("mock://") {
            return match scenario {
                "success" => Ok(SendResult {
                    provider_message_id: "mock-ts".into(),
                    delivered: true,
                    raw: payload,
                }),
                "throttle" => Err(MsgError::retryable(
                    "slack_retryable",
                    "mock throttled",
                    Some(1_000),
                )),
                other => Err(MsgError::permanent(
                    "slack_mock",
                    format!("unknown mock scenario `{other}`"),
                )),
            };
        }

        let response = self
            .client
            .post(&endpoint)
            .bearer_auth(token)
            .json(&payload)
            .send()
            .await
            .map_err(|err| {
                MsgError::retryable("slack_http", "failed to call Slack API", Some(1_000))
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
            MsgError::retryable("slack_body", "failed to read response body", Some(1_000))
                .with_source(err)
        })?;
        let raw: Value = serde_json::from_str(&body_text).unwrap_or(Value::Null);

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(MsgError::retryable(
                "slack_retryable",
                format!("status={} body={}", status.as_u16(), body_text),
                retry_after,
            ));
        }

        if status.is_server_error() {
            return Err(MsgError::retryable(
                "slack_retryable",
                format!("status={} body={}", status.as_u16(), body_text),
                retry_after.or(Some(1_000)),
            ));
        }

        if status.is_client_error() {
            return Err(MsgError::permanent(
                "slack_send_failed",
                format!("status={} body={}", status.as_u16(), body_text),
            ));
        }

        let response: SlackSendResponse = serde_json::from_value(raw.clone()).map_err(|err| {
            MsgError::permanent("slack_response", "invalid Slack response").with_source(err)
        })?;

        if !response.ok {
            let err_code = response.error.unwrap_or_else(|| "unknown".into());
            return Err(MsgError::permanent(
                "slack_api_error",
                format!("slack error: {err_code}"),
            ));
        }

        let ts = response
            .ts
            .ok_or_else(|| MsgError::permanent("slack_missing_ts", "timestamp missing"))?;

        Ok(SendResult {
            provider_message_id: ts,
            delivered: true,
            raw,
        })
    }
}

#[derive(Deserialize)]
struct SlackSendResponse {
    ok: bool,
    ts: Option<String>,
    error: Option<String>,
}

#[derive(Default)]
struct SlackReceiveAdapter;

#[derive(Default, Debug, Deserialize, Serialize)]
struct SlackEventWrapper {
    #[serde(default)]
    r#type: Option<String>,
    #[serde(default)]
    event: Option<SlackEvent>,
}

#[derive(Default, Debug, Deserialize, Serialize)]
struct SlackEvent {
    #[serde(default, rename = "type")]
    event_type: Option<String>,
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    user: Option<String>,
    #[serde(default)]
    thread_ts: Option<String>,
    #[serde(default)]
    ts: Option<String>,
}

impl ReceiveAdapter for SlackReceiveAdapter {
    fn ingest(&self, _ctx: &TenantCtx, payload: Value) -> Result<Vec<Message>, MsgError> {
        let wrapper: SlackEventWrapper = serde_json::from_value(payload).map_err(|err| {
            MsgError::permanent("slack_webhook", "invalid slack event payload").with_source(err)
        })?;

        let event = match wrapper.event {
            Some(ev) => ev,
            None => return Ok(Vec::new()),
        };

        if event
            .event_type
            .as_deref()
            .map(|t| t != "message")
            .unwrap_or(true)
        {
            return Ok(Vec::new());
        }

        let metadata = serde_json::to_value(&event).unwrap_or(Value::Null);
        let SlackEvent {
            channel,
            text,
            user,
            thread_ts,
            ..
        } = event;

        let text = text.unwrap_or_default();
        if text.trim().is_empty() {
            return Ok(Vec::new());
        }

        let message = Message {
            chat_id: channel,
            thread_id: thread_ts,
            user_id: user,
            text: Some(text),
            metadata,
            ..Default::default()
        };

        Ok(vec![message])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gsm_core::make_tenant_ctx;
    use std::sync::OnceLock;
    use tokio::sync::Mutex;

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
        let _guard = env_lock().lock().await;
        let prev_token = std::env::var("SLACK_BOT_TOKEN").ok();
        unsafe {
            std::env::set_var("SLACK_BOT_TOKEN", "xoxb-123");
        }
        let prev_url = std::env::var("SLACK_SEND_URL").ok();
        unsafe {
            std::env::set_var("SLACK_SEND_URL", "mock://success");
        }

        let manifest = ProviderManifest::from_json(MANIFEST_STR).unwrap();
        let adapter = SlackSendAdapter::from_manifest(&manifest).unwrap();

        let ctx = make_tenant_ctx("acme".into(), None, None);
        let message = Message {
            chat_id: Some("C123".into()),
            text: Some("hello".into()),
            ..Default::default()
        };

        let result = adapter.send(&ctx, &message).await.unwrap();
        assert_eq!(result.provider_message_id, "mock-ts");
        assert!(result.delivered);

        restore_env("SLACK_BOT_TOKEN", prev_token);
        restore_env("SLACK_SEND_URL", prev_url);
    }

    #[tokio::test]
    async fn send_handles_retryable() {
        let _guard = env_lock().lock().await;
        let prev_token = std::env::var("SLACK_BOT_TOKEN").ok();
        unsafe {
            std::env::set_var("SLACK_BOT_TOKEN", "xoxb-123");
        }
        let prev_url = std::env::var("SLACK_SEND_URL").ok();
        unsafe {
            std::env::set_var("SLACK_SEND_URL", "mock://throttle");
        }

        let manifest = ProviderManifest::from_json(MANIFEST_STR).unwrap();
        let adapter = SlackSendAdapter::from_manifest(&manifest).unwrap();

        let ctx = make_tenant_ctx("acme".into(), None, None);
        let message = Message {
            chat_id: Some("C123".into()),
            text: Some("hello".into()),
            ..Default::default()
        };

        let err = adapter.send(&ctx, &message).await.err().unwrap();
        assert!(err.is_retryable());

        restore_env("SLACK_BOT_TOKEN", prev_token);
        restore_env("SLACK_SEND_URL", prev_url);
    }

    #[test]
    fn receive_parses_event() {
        let adapter = SlackReceiveAdapter;
        let ctx = make_tenant_ctx("acme".into(), None, None);
        let payload = json!({
            "type": "event_callback",
            "event": {
                "type": "message",
                "channel": "C123",
                "text": "hi there",
                "user": "U123",
                "thread_ts": "1680000000.000100"
            }
        });

        let messages = adapter.ingest(&ctx, payload).unwrap();
        assert_eq!(messages.len(), 1);
        let msg = &messages[0];
        assert_eq!(msg.chat_id.as_deref(), Some("C123"));
        assert_eq!(msg.thread_id.as_deref(), Some("1680000000.000100"));
        assert_eq!(msg.user_id.as_deref(), Some("U123"));
        assert_eq!(msg.text.as_deref(), Some("hi there"));
    }
}
