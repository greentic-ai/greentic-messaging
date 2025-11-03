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
    "/../../providers/whatsapp/provider.json"
));

pub fn register(registry: &mut ProviderRegistry) -> Result<(), anyhow::Error> {
    let manifest = ProviderManifest::from_json(MANIFEST_STR)?;
    let manifest_for_send = Arc::new(manifest.clone());
    let builder = ProviderBuilder::new()
        .with_send(move || {
            let manifest = Arc::clone(&manifest_for_send);
            WhatsappSendAdapter::from_manifest(&manifest)
                .map(|adapter| Box::new(adapter) as Box<dyn SendAdapter>)
        })
        .with_receive(|| Ok(Box::new(WhatsappReceiveAdapter) as Box<dyn ReceiveAdapter>));

    registry
        .register(manifest, builder)
        .map_err(|err| anyhow!(err))
}

struct WhatsappSendAdapter {
    client: reqwest::Client,
    endpoint_template: String,
    default_token: Option<String>,
    default_phone_id: Option<String>,
}

impl WhatsappSendAdapter {
    fn from_manifest(manifest: &ProviderManifest) -> Result<Self, MsgError> {
        let client = reqwest::Client::builder()
            .user_agent("gsm-whatsapp-provider/0.1")
            .build()
            .map_err(|err| {
                MsgError::permanent("whatsapp_client", "failed to create HTTP client")
                    .with_source(err)
            })?;

        Ok(Self {
            client,
            endpoint_template: manifest.endpoints.send.clone(),
            default_token: std::env::var("WHATSAPP_TOKEN").ok(),
            default_phone_id: std::env::var("WHATSAPP_PHONE_ID").ok(),
        })
    }

    fn resolve_token(&self) -> Result<String, MsgError> {
        if let Ok(value) = std::env::var("WHATSAPP_TOKEN")
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
            "whatsapp_missing_token",
            "WHATSAPP_TOKEN is not configured",
        ))
    }

    fn resolve_phone_id(&self) -> Result<String, MsgError> {
        if let Ok(value) = std::env::var("WHATSAPP_PHONE_ID")
            && !value.trim().is_empty()
        {
            return Ok(value);
        }
        if let Some(value) = self.default_phone_id.as_ref()
            && !value.trim().is_empty()
        {
            return Ok(value.clone());
        }
        Err(MsgError::permanent(
            "whatsapp_missing_phone_id",
            "WHATSAPP_PHONE_ID is not configured",
        ))
    }

    fn endpoint(&self, phone_id: &str) -> String {
        std::env::var("WHATSAPP_SEND_URL")
            .unwrap_or_else(|_| self.endpoint_template.replace("{PHONE_ID}", phone_id))
    }

    fn build_payload(&self, message: &Message) -> Result<Value, MsgError> {
        let to = message.to.clone().ok_or_else(|| {
            MsgError::permanent("whatsapp_missing_to", "recipient phone number required")
        })?;

        let text = message.text.clone().unwrap_or_default();
        if text.trim().is_empty() {
            return Err(MsgError::permanent(
                "whatsapp_missing_text",
                "message text cannot be empty",
            ));
        }

        Ok(json!({
            "messaging_product": "whatsapp",
            "to": to,
            "type": "text",
            "text": {
                "preview_url": false,
                "body": text
            }
        }))
    }
}

#[async_trait]
impl SendAdapter for WhatsappSendAdapter {
    async fn send(&self, _ctx: &TenantCtx, message: &Message) -> Result<SendResult, MsgError> {
        let token = self.resolve_token()?;
        let phone_id = self.resolve_phone_id()?;
        let payload = self.build_payload(message)?;
        let endpoint = self.endpoint(&phone_id);

        if let Some(scenario) = endpoint.strip_prefix("mock://") {
            return match scenario {
                "success" => Ok(SendResult {
                    provider_message_id: "mock-msg".into(),
                    delivered: true,
                    raw: payload,
                }),
                "throttle" => Err(MsgError::retryable(
                    "whatsapp_retryable",
                    "mock throttled",
                    Some(1_000),
                )),
                other => Err(MsgError::permanent(
                    "whatsapp_mock",
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
                MsgError::retryable("whatsapp_http", "failed to call WhatsApp API", Some(1_000))
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
            MsgError::retryable("whatsapp_body", "failed to read response body", Some(1_000))
                .with_source(err)
        })?;
        let raw: Value = serde_json::from_str(&body_text).unwrap_or(Value::Null);

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(MsgError::retryable(
                "whatsapp_retryable",
                format!("status={} body={}", status.as_u16(), body_text),
                retry_after,
            ));
        }

        if status.is_server_error() {
            return Err(MsgError::retryable(
                "whatsapp_retryable",
                format!("status={} body={}", status.as_u16(), body_text),
                retry_after.or(Some(1_000)),
            ));
        }

        if status.is_client_error() {
            return Err(MsgError::permanent(
                "whatsapp_send_failed",
                format!("status={} body={}", status.as_u16(), body_text),
            ));
        }

        let response: WhatsappSendResponse =
            serde_json::from_value(raw.clone()).map_err(|err| {
                MsgError::permanent("whatsapp_response", "invalid WhatsApp response")
                    .with_source(err)
            })?;

        let message_id = response
            .messages
            .and_then(|mut msgs| msgs.pop())
            .map(|msg| msg.id)
            .ok_or_else(|| MsgError::permanent("whatsapp_missing_id", "response missing id"))?;

        Ok(SendResult {
            provider_message_id: message_id,
            delivered: true,
            raw,
        })
    }
}

#[derive(Deserialize)]
struct WhatsappSendResponse {
    messages: Option<Vec<WhatsappMessageInfo>>,
}

#[derive(Deserialize)]
struct WhatsappMessageInfo {
    id: String,
}

#[derive(Default)]
struct WhatsappReceiveAdapter;

#[derive(Default, Debug, Deserialize, Serialize)]
struct WhatsappWebhook {
    #[serde(default)]
    entry: Vec<WhatsappEntry>,
}

#[derive(Default, Debug, Deserialize, Serialize)]
struct WhatsappEntry {
    #[serde(default)]
    changes: Vec<WhatsappChange>,
}

#[derive(Default, Debug, Deserialize, Serialize)]
struct WhatsappChange {
    #[serde(default)]
    value: WhatsappValue,
}

#[derive(Default, Debug, Deserialize, Serialize)]
struct WhatsappValue {
    #[serde(default)]
    messages: Vec<WhatsappIncomingMessage>,
}

#[derive(Default, Debug, Deserialize, Serialize)]
struct WhatsappIncomingMessage {
    #[serde(default)]
    from: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    r#type: Option<String>,
    #[serde(default)]
    text: Option<WhatsappText>,
}

#[derive(Default, Debug, Deserialize, Serialize)]
struct WhatsappText {
    #[serde(default)]
    body: Option<String>,
}

impl ReceiveAdapter for WhatsappReceiveAdapter {
    fn ingest(&self, _ctx: &TenantCtx, payload: Value) -> Result<Vec<Message>, MsgError> {
        let webhook: WhatsappWebhook = serde_json::from_value(payload).map_err(|err| {
            MsgError::permanent("whatsapp_webhook", "invalid webhook payload").with_source(err)
        })?;

        let mut messages = Vec::new();
        for entry in webhook.entry {
            for change in entry.changes {
                for msg in change.value.messages {
                    if msg.r#type.as_deref() != Some("text") {
                        continue;
                    }
                    let text = msg
                        .text
                        .as_ref()
                        .and_then(|t| t.body.clone())
                        .unwrap_or_default();
                    if text.trim().is_empty() {
                        continue;
                    }

                    let metadata = serde_json::to_value(&msg).unwrap_or(Value::Null);
                    messages.push(Message {
                        chat_id: msg.from.clone(),
                        user_id: msg.from.clone(),
                        text: Some(text),
                        metadata,
                        ..Default::default()
                    });
                }
            }
        }

        Ok(messages)
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
        let prev_token = std::env::var("WHATSAPP_TOKEN").ok();
        unsafe {
            std::env::set_var("WHATSAPP_TOKEN", "token-123");
        }
        let prev_phone = std::env::var("WHATSAPP_PHONE_ID").ok();
        unsafe {
            std::env::set_var("WHATSAPP_PHONE_ID", "phone-1");
        }
        let prev_url = std::env::var("WHATSAPP_SEND_URL").ok();
        unsafe {
            std::env::set_var("WHATSAPP_SEND_URL", "mock://success");
        }

        let manifest = ProviderManifest::from_json(MANIFEST_STR).unwrap();
        let adapter = WhatsappSendAdapter::from_manifest(&manifest).unwrap();

        let ctx = make_tenant_ctx("acme".into(), None, None);
        let message = Message {
            to: Some("+15551234567".into()),
            text: Some("hello".into()),
            ..Default::default()
        };

        let result = adapter.send(&ctx, &message).await.unwrap();
        assert_eq!(result.provider_message_id, "mock-msg");
        assert!(result.delivered);

        restore_env("WHATSAPP_TOKEN", prev_token);
        restore_env("WHATSAPP_PHONE_ID", prev_phone);
        restore_env("WHATSAPP_SEND_URL", prev_url);
    }

    #[tokio::test]
    async fn send_handles_retryable() {
        let _guard = env_lock().lock().await;
        let prev_token = std::env::var("WHATSAPP_TOKEN").ok();
        unsafe {
            std::env::set_var("WHATSAPP_TOKEN", "token-123");
        }
        let prev_phone = std::env::var("WHATSAPP_PHONE_ID").ok();
        unsafe {
            std::env::set_var("WHATSAPP_PHONE_ID", "phone-1");
        }
        let prev_url = std::env::var("WHATSAPP_SEND_URL").ok();
        unsafe {
            std::env::set_var("WHATSAPP_SEND_URL", "mock://throttle");
        }

        let manifest = ProviderManifest::from_json(MANIFEST_STR).unwrap();
        let adapter = WhatsappSendAdapter::from_manifest(&manifest).unwrap();

        let ctx = make_tenant_ctx("acme".into(), None, None);
        let message = Message {
            to: Some("+15551234567".into()),
            text: Some("hello".into()),
            ..Default::default()
        };

        let err = adapter.send(&ctx, &message).await.err().unwrap();
        assert!(err.is_retryable());

        restore_env("WHATSAPP_TOKEN", prev_token);
        restore_env("WHATSAPP_PHONE_ID", prev_phone);
        restore_env("WHATSAPP_SEND_URL", prev_url);
    }

    #[test]
    fn receive_parses_messages() {
        let adapter = WhatsappReceiveAdapter;
        let ctx = make_tenant_ctx("acme".into(), None, None);
        let payload = json!({
            "entry": [{
                "changes": [{
                    "value": {
                        "messages": [{
                            "from": "15550001111",
                            "type": "text",
                            "text": { "body": "Hello" }
                        }]
                    }
                }]
            }]
        });

        let messages = adapter.ingest(&ctx, payload).unwrap();
        assert_eq!(messages.len(), 1);
        let msg = &messages[0];
        assert_eq!(msg.chat_id.as_deref(), Some("15550001111"));
        assert_eq!(msg.text.as_deref(), Some("Hello"));
    }
}
