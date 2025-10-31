use std::sync::Arc;

use anyhow::anyhow;
use async_trait::async_trait;
use gsm_core::TenantCtx;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::errors::MsgError;
use crate::manifest::ProviderManifest;
use crate::registry::{ProviderBuilder, ProviderRegistry};
use crate::traits::{Message, ReceiveAdapter, SendAdapter, SendResult};

const MANIFEST_STR: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../providers/webex/provider.json"
));

pub fn register(registry: &mut ProviderRegistry) -> Result<(), anyhow::Error> {
    let manifest = ProviderManifest::from_json(MANIFEST_STR)?;
    let manifest_for_send = Arc::new(manifest.clone());
    let builder = ProviderBuilder::new()
        .with_send(move || {
            let manifest = Arc::clone(&manifest_for_send);
            WebexSendAdapter::from_manifest(&manifest)
                .map(|adapter| Box::new(adapter) as Box<dyn SendAdapter>)
        })
        .with_receive(|| Ok(Box::new(WebexReceiveAdapter::default()) as Box<dyn ReceiveAdapter>));

    registry
        .register(manifest, builder)
        .map_err(|err| anyhow!(err))
}

struct WebexSendAdapter {
    client: reqwest::Client,
    endpoint: String,
    default_token: Option<String>,
}

impl WebexSendAdapter {
    fn from_manifest(manifest: &ProviderManifest) -> Result<Self, MsgError> {
        let client = reqwest::Client::builder()
            .user_agent("gsm-webex-provider/0.1")
            .build()
            .map_err(|err| {
                MsgError::permanent("webex_client", "failed to create HTTP client").with_source(err)
            })?;

        let default_token = std::env::var("WEBEX_BOT_TOKEN").ok();
        Ok(Self {
            client,
            endpoint: manifest.endpoints.send.clone(),
            default_token,
        })
    }

    fn resolve_token(&self) -> Result<String, MsgError> {
        if let Ok(value) = std::env::var("WEBEX_BOT_TOKEN") {
            if !value.trim().is_empty() {
                return Ok(value);
            }
        }
        if let Some(value) = &self.default_token {
            if !value.trim().is_empty() {
                return Ok(value.clone());
            }
        }
        Err(MsgError::permanent(
            "webex_missing_token",
            "WEBEX_BOT_TOKEN is not configured",
        ))
    }

    fn endpoint(&self) -> String {
        std::env::var("WEBEX_SEND_URL").unwrap_or_else(|_| self.endpoint.clone())
    }

    fn build_payload(&self, message: &Message) -> Result<Value, MsgError> {
        let text = message.text.clone().unwrap_or_default();
        let mut payload = json!({ "text": text });

        if let Some(room) = message.chat_id.clone() {
            payload
                .as_object_mut()
                .expect("payload is object")
                .insert("roomId".into(), Value::String(room));
        } else if let Some(email) = message.to.clone() {
            payload
                .as_object_mut()
                .expect("payload is object")
                .insert("toPersonEmail".into(), Value::String(email));
        } else {
            return Err(MsgError::permanent(
                "webex_missing_destination",
                "chat_id or recipient email required",
            ));
        }

        Ok(payload)
    }
}

#[async_trait]
impl SendAdapter for WebexSendAdapter {
    async fn send(&self, _ctx: &TenantCtx, message: &Message) -> Result<SendResult, MsgError> {
        let token = self.resolve_token()?;
        let payload = self.build_payload(message)?;
        let endpoint = self.endpoint();

        if let Some(scenario) = endpoint.strip_prefix("mock://") {
            return match scenario {
                "success" => Ok(SendResult {
                    provider_message_id: "mock-message".into(),
                    delivered: true,
                    raw: payload,
                }),
                "throttle" => Err(MsgError::retryable(
                    "webex_retryable",
                    "mock throttled",
                    Some(1_000),
                )),
                other => Err(MsgError::permanent(
                    "webex_mock",
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
                MsgError::retryable("webex_http", "failed to call Webex API", Some(1_000))
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
            MsgError::retryable("webex_body", "failed to read response body", Some(1_000))
                .with_source(err)
        })?;
        let raw: Value = serde_json::from_str(&body_text).unwrap_or(Value::Null);

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(MsgError::retryable(
                "webex_retryable",
                format!("status={} body={}", status.as_u16(), body_text),
                retry_after,
            ));
        }

        if status.is_server_error() {
            return Err(MsgError::retryable(
                "webex_retryable",
                format!("status={} body={}", status.as_u16(), body_text),
                retry_after.or(Some(1_000)),
            ));
        }

        if status.is_client_error() {
            return Err(MsgError::permanent(
                "webex_send_failed",
                format!("status={} body={}", status.as_u16(), body_text),
            ));
        }

        let response: WebexSendResponse = serde_json::from_value(raw.clone()).map_err(|err| {
            MsgError::permanent("webex_response", "invalid Webex response").with_source(err)
        })?;

        Ok(SendResult {
            provider_message_id: response.id,
            delivered: true,
            raw,
        })
    }
}

#[derive(Deserialize)]
struct WebexSendResponse {
    id: String,
}

#[derive(Default)]
struct WebexReceiveAdapter;

#[derive(Debug, Default, Deserialize, Serialize)]
struct WebexWebhook {
    #[serde(default)]
    data: WebexWebhookData,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct WebexWebhookData {
    #[serde(rename = "id")]
    message_id: Option<String>,
    #[serde(rename = "roomId")]
    room_id: Option<String>,
    #[serde(rename = "personId")]
    person_id: Option<String>,
    #[serde(rename = "personEmail")]
    person_email: Option<String>,
    text: Option<String>,
    created: Option<String>,
}

impl ReceiveAdapter for WebexReceiveAdapter {
    fn ingest(&self, _ctx: &TenantCtx, payload: Value) -> Result<Vec<Message>, MsgError> {
        let webhook: WebexWebhook = serde_json::from_value(payload).map_err(|err| {
            MsgError::permanent("webex_webhook", "invalid webhook payload").with_source(err)
        })?;

        let data = webhook.data;
        if data.room_id.is_none() && data.person_email.is_none() {
            return Ok(Vec::new());
        }

        let metadata = serde_json::to_value(&data).unwrap_or(Value::Null);
        let message = Message {
            chat_id: data.room_id,
            user_id: data.person_id.or(data.person_email.clone()),
            text: data.text,
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

    fn restore_env(key: &str, previous: Option<String>) {
        if let Some(value) = previous {
            std::env::set_var(key, value);
        } else {
            std::env::remove_var(key);
        }
    }

    #[tokio::test]
    async fn send_succeeds() {
        let prev_token = std::env::var("WEBEX_BOT_TOKEN").ok();
        std::env::set_var("WEBEX_BOT_TOKEN", "token-123");
        let prev_url = std::env::var("WEBEX_SEND_URL").ok();
        std::env::set_var("WEBEX_SEND_URL", "mock://success");

        let manifest = ProviderManifest::from_json(MANIFEST_STR).unwrap();
        let adapter = WebexSendAdapter::from_manifest(&manifest).unwrap();

        let ctx = make_tenant_ctx("acme".into(), None, None);
        let message = Message {
            chat_id: Some("room-1".into()),
            text: Some("hello".into()),
            ..Default::default()
        };

        let result = adapter.send(&ctx, &message).await.unwrap();
        assert_eq!(result.provider_message_id, "mock-message");
        assert!(result.delivered);

        restore_env("WEBEX_BOT_TOKEN", prev_token);
        restore_env("WEBEX_SEND_URL", prev_url);
    }

    #[tokio::test]
    async fn send_handles_retryable() {
        let prev_token = std::env::var("WEBEX_BOT_TOKEN").ok();
        std::env::set_var("WEBEX_BOT_TOKEN", "token-123");
        let prev_url = std::env::var("WEBEX_SEND_URL").ok();
        std::env::set_var("WEBEX_SEND_URL", "mock://throttle");

        let manifest = ProviderManifest::from_json(MANIFEST_STR).unwrap();
        let adapter = WebexSendAdapter::from_manifest(&manifest).unwrap();

        let ctx = make_tenant_ctx("acme".into(), None, None);
        let message = Message {
            chat_id: Some("room-1".into()),
            text: Some("hello".into()),
            ..Default::default()
        };

        let err = adapter.send(&ctx, &message).await.err().unwrap();
        assert!(err.is_retryable());

        restore_env("WEBEX_BOT_TOKEN", prev_token);
        restore_env("WEBEX_SEND_URL", prev_url);
    }

    #[test]
    fn receive_parses_webhook() {
        let adapter = WebexReceiveAdapter::default();
        let ctx = make_tenant_ctx("acme".into(), None, None);
        let payload = json!({
            "id": "abc",
            "resource": "messages",
            "event": "created",
            "data": {
                "id": "msg-1",
                "roomId": "room-1",
                "personId": "person-1",
                "personEmail": "user@example.com",
                "text": "hello",
                "created": "2024-01-01T00:00:00Z"
            }
        });

        let messages = adapter.ingest(&ctx, payload).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].chat_id.as_deref(), Some("room-1"));
        assert_eq!(messages[0].user_id.as_deref(), Some("person-1"));
        assert_eq!(messages[0].text.as_deref(), Some("hello"));
    }
}
