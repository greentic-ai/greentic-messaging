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
    "/../../providers/teams/provider.json"
));

pub fn register(registry: &mut ProviderRegistry) -> Result<(), anyhow::Error> {
    let manifest = ProviderManifest::from_json(MANIFEST_STR)?;
    let manifest_for_send = Arc::new(manifest.clone());
    let builder = ProviderBuilder::new()
        .with_send(move || {
            let manifest = Arc::clone(&manifest_for_send);
            TeamsSendAdapter::from_manifest(&manifest)
                .map(|adapter| Box::new(adapter) as Box<dyn SendAdapter>)
        })
        .with_receive(|| Ok(Box::new(TeamsReceiveAdapter::default()) as Box<dyn ReceiveAdapter>));

    registry
        .register(manifest, builder)
        .map_err(|err| anyhow!(err))
}

struct TeamsSendAdapter {
    client: reqwest::Client,
    manifest: ProviderManifest,
    default_tenant_id: Option<String>,
    default_client_id: Option<String>,
    default_client_secret: Option<String>,
}

impl TeamsSendAdapter {
    fn from_manifest(manifest: &ProviderManifest) -> Result<Self, MsgError> {
        let client = reqwest::Client::builder()
            .user_agent("gsm-teams-provider/0.1")
            .build()
            .map_err(|err| {
                MsgError::permanent("teams_client", "failed to create HTTP client").with_source(err)
            })?;

        Ok(Self {
            client,
            manifest: manifest.clone(),
            default_tenant_id: std::env::var("MS_GRAPH_TENANT_ID").ok(),
            default_client_id: std::env::var("MS_GRAPH_CLIENT_ID").ok(),
            default_client_secret: std::env::var("MS_GRAPH_CLIENT_SECRET").ok(),
        })
    }

    fn resolve_secret(name: &str, default: &Option<String>) -> Result<String, MsgError> {
        if let Ok(value) = std::env::var(name) {
            if !value.trim().is_empty() {
                return Ok(value);
            }
        }
        if let Some(value) = default {
            if !value.trim().is_empty() {
                return Ok(value.clone());
            }
        }
        Err(MsgError::permanent(
            "teams_missing_secret",
            format!("{name} is not configured"),
        ))
    }

    async fn access_token(&self) -> Result<String, MsgError> {
        let tenant_id = Self::resolve_secret("MS_GRAPH_TENANT_ID", &self.default_tenant_id)?;
        if tenant_id.starts_with("mock://") {
            return Ok("mock-token".into());
        }
        let client_id = Self::resolve_secret("MS_GRAPH_CLIENT_ID", &self.default_client_id)?;
        let client_secret =
            Self::resolve_secret("MS_GRAPH_CLIENT_SECRET", &self.default_client_secret)?;

        let token_url = format!("https://login.microsoftonline.com/{tenant_id}/oauth2/v2.0/token");

        if token_url.starts_with("mock://") {
            return Ok("mock-token".into());
        }

        let params = [
            ("grant_type", "client_credentials"),
            ("client_id", client_id.as_str()),
            ("client_secret", client_secret.as_str()),
            ("scope", "https://graph.microsoft.com/.default"),
        ];

        let response = self
            .client
            .post(token_url)
            .form(&params)
            .send()
            .await
            .map_err(|err| {
                MsgError::retryable(
                    "teams_token_http",
                    "failed to obtain teams token",
                    Some(1_000),
                )
                .with_source(err)
            })?;

        let status = response.status();
        let body_text = response.text().await.map_err(|err| {
            MsgError::retryable(
                "teams_token_body",
                "failed to read token response",
                Some(1_000),
            )
            .with_source(err)
        })?;

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(MsgError::retryable(
                "teams_token_retry",
                format!("status={} body={}", status.as_u16(), body_text),
                Some(1_000),
            ));
        }

        if status.is_client_error() {
            return Err(MsgError::permanent(
                "teams_token_failed",
                format!("status={} body={}", status.as_u16(), body_text),
            ));
        }

        if status.is_server_error() {
            return Err(MsgError::retryable(
                "teams_token_retry",
                format!("status={} body={}", status.as_u16(), body_text),
                Some(1_000),
            ));
        }

        let response: TeamsTokenResponse = serde_json::from_str(&body_text).map_err(|err| {
            MsgError::permanent("teams_token_response", "invalid token response").with_source(err)
        })?;

        response
            .access_token
            .ok_or_else(|| MsgError::permanent("teams_token_missing", "missing access token"))
    }

    fn endpoint_for(&self, message: &Message) -> Result<String, MsgError> {
        if let (Some(team_id), Some(channel_id)) = (
            message.metadata.get("team_id").and_then(|v| v.as_str()),
            message.metadata.get("channel_id").and_then(|v| v.as_str()),
        ) {
            return Ok(format!(
                "{}/teams/{team_id}/channels/{channel_id}/messages",
                self.manifest.endpoints.send.trim_end_matches('/')
            ));
        }

        if let Some(chat_id) = message.metadata.get("chat_id").and_then(|v| v.as_str()) {
            return Ok(format!(
                "{}/chats/{chat_id}/messages",
                self.manifest.endpoints.send.trim_end_matches('/')
            ));
        }

        Err(MsgError::permanent(
            "teams_missing_destination",
            "team/channel or chat identifier required",
        ))
    }

    fn build_payload(&self, message: &Message) -> Result<Value, MsgError> {
        let text = message.text.clone().unwrap_or_default();
        if text.trim().is_empty() {
            return Err(MsgError::permanent(
                "teams_missing_text",
                "message text cannot be empty",
            ));
        }

        let mut payload = json!({
            "body": {
                "contentType": "html",
                "content": text
            }
        });

        if let Some(thread) = message.thread_id.clone() {
            payload
                .as_object_mut()
                .expect("payload object")
                .insert("replyToId".into(), Value::String(thread));
        }

        Ok(payload)
    }
}

#[async_trait]
impl SendAdapter for TeamsSendAdapter {
    async fn send(&self, _ctx: &TenantCtx, message: &Message) -> Result<SendResult, MsgError> {
        let token = self.access_token().await?;
        let endpoint = self.endpoint_for(message)?;
        let payload = self.build_payload(message)?;

        if endpoint.starts_with("mock://") {
            let rest = endpoint.trim_start_matches("mock://");
            let scenario = rest.splitn(2, '/').next().unwrap_or(rest);
            return match scenario {
                "success" => Ok(SendResult {
                    provider_message_id: "mock-message".into(),
                    delivered: true,
                    raw: payload,
                }),
                "throttle" => Err(MsgError::retryable(
                    "teams_retryable",
                    "mock throttled",
                    Some(1_000),
                )),
                other => Err(MsgError::permanent(
                    "teams_mock",
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
                MsgError::retryable("teams_http", "failed to call teams API", Some(1_000))
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
            MsgError::retryable("teams_body", "failed to read response body", Some(1_000))
                .with_source(err)
        })?;
        let raw: Value = serde_json::from_str(&body_text).unwrap_or(Value::Null);

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(MsgError::retryable(
                "teams_retryable",
                format!("status={} body={}", status.as_u16(), body_text),
                retry_after,
            ));
        }

        if status.is_server_error() {
            return Err(MsgError::retryable(
                "teams_retryable",
                format!("status={} body={}", status.as_u16(), body_text),
                retry_after.or(Some(1_000)),
            ));
        }

        if status.is_client_error() {
            return Err(MsgError::permanent(
                "teams_send_failed",
                format!("status={} body={}", status.as_u16(), body_text),
            ));
        }

        let response: TeamsSendResponse = serde_json::from_value(raw.clone()).map_err(|err| {
            MsgError::permanent("teams_response", "invalid teams response").with_source(err)
        })?;

        let message_id = response
            .id
            .ok_or_else(|| MsgError::permanent("teams_missing_id", "response missing id"))?;

        Ok(SendResult {
            provider_message_id: message_id,
            delivered: true,
            raw,
        })
    }
}

#[derive(Deserialize)]
struct TeamsTokenResponse {
    access_token: Option<String>,
}

#[derive(Deserialize)]
struct TeamsSendResponse {
    id: Option<String>,
}

#[derive(Default)]
struct TeamsReceiveAdapter;

#[derive(Default, Debug, Deserialize, Serialize)]
struct TeamsMessagePayload {
    #[serde(rename = "id", default)]
    id: Option<String>,
    #[serde(rename = "body", default)]
    body: Option<TeamsMessageBody>,
    #[serde(rename = "from", default)]
    from: Option<TeamsMessageFrom>,
    #[serde(rename = "channelIdentity", default)]
    channel_identity: Option<TeamsChannelIdentity>,
    #[serde(rename = "messageType", default)]
    message_type: Option<String>,
    #[serde(rename = "replyToId", default)]
    reply_to_id: Option<String>,
}

#[derive(Default, Debug, Deserialize, Serialize)]
struct TeamsMessageBody {
    #[serde(rename = "content", default)]
    content: Option<String>,
}

#[derive(Default, Debug, Deserialize, Serialize)]
struct TeamsMessageFrom {
    #[serde(rename = "user", default)]
    user: Option<TeamsUser>,
}

#[derive(Default, Debug, Deserialize, Serialize)]
struct TeamsUser {
    #[serde(rename = "id", default)]
    id: Option<String>,
}

#[derive(Default, Debug, Deserialize, Serialize)]
struct TeamsChannelIdentity {
    #[serde(rename = "teamId", default)]
    team_id: Option<String>,
    #[serde(rename = "channelId", default)]
    channel_id: Option<String>,
}

impl ReceiveAdapter for TeamsReceiveAdapter {
    fn ingest(&self, _ctx: &TenantCtx, payload: Value) -> Result<Vec<Message>, MsgError> {
        let message: TeamsMessagePayload = serde_json::from_value(payload).map_err(|err| {
            MsgError::permanent("teams_webhook", "invalid teams message payload").with_source(err)
        })?;

        let text = message
            .body
            .as_ref()
            .and_then(|b| b.content.clone())
            .unwrap_or_default();
        if text.trim().is_empty() {
            return Ok(Vec::new());
        }

        let metadata = serde_json::to_value(&message).unwrap_or(Value::Null);
        let message = Message {
            chat_id: message.channel_identity.and_then(|ci| ci.channel_id),
            thread_id: message.reply_to_id,
            user_id: message
                .from
                .and_then(|from| from.user)
                .and_then(|user| user.id),
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
        let prev_tenant = std::env::var("MS_GRAPH_TENANT_ID").ok();
        unsafe {
            std::env::set_var("MS_GRAPH_TENANT_ID", "mock://tenant");
        }
        let prev_client = std::env::var("MS_GRAPH_CLIENT_ID").ok();
        unsafe {
            std::env::set_var("MS_GRAPH_CLIENT_ID", "mock_client");
        }
        let prev_secret = std::env::var("MS_GRAPH_CLIENT_SECRET").ok();
        unsafe {
            std::env::set_var("MS_GRAPH_CLIENT_SECRET", "mock_secret");
        }
        let manifest = ProviderManifest::from_json(MANIFEST_STR).unwrap();
        let mut adapter = TeamsSendAdapter::from_manifest(&manifest).unwrap();
        adapter.manifest.endpoints.send = "mock://success".into();

        let ctx = make_tenant_ctx("acme".into(), None, None);
        let metadata = json!({ "team_id": "team-1", "channel_id": "channel-1" });
        let message = Message {
            metadata,
            text: Some("hello".into()),
            ..Default::default()
        };

        let result = adapter.send(&ctx, &message).await.unwrap();
        assert_eq!(result.provider_message_id, "mock-message");
        assert!(result.delivered);

        restore_env("MS_GRAPH_TENANT_ID", prev_tenant);
        restore_env("MS_GRAPH_CLIENT_ID", prev_client);
        restore_env("MS_GRAPH_CLIENT_SECRET", prev_secret);
    }

    #[tokio::test]
    async fn send_handles_retryable() {
        let prev_tenant = std::env::var("MS_GRAPH_TENANT_ID").ok();
        unsafe {
            std::env::set_var("MS_GRAPH_TENANT_ID", "mock://tenant");
        }
        let prev_client = std::env::var("MS_GRAPH_CLIENT_ID").ok();
        unsafe {
            std::env::set_var("MS_GRAPH_CLIENT_ID", "mock_client");
        }
        let prev_secret = std::env::var("MS_GRAPH_CLIENT_SECRET").ok();
        unsafe {
            std::env::set_var("MS_GRAPH_CLIENT_SECRET", "mock_secret");
        }
        let manifest = ProviderManifest::from_json(MANIFEST_STR).unwrap();
        let mut adapter = TeamsSendAdapter::from_manifest(&manifest).unwrap();
        adapter.manifest.endpoints.send = "mock://throttle".into();

        let ctx = make_tenant_ctx("acme".into(), None, None);
        let metadata = json!({ "team_id": "team-1", "channel_id": "channel-1" });
        let message = Message {
            metadata,
            text: Some("hello".into()),
            ..Default::default()
        };

        let err = adapter.send(&ctx, &message).await.err().unwrap();
        assert!(err.is_retryable());

        restore_env("MS_GRAPH_TENANT_ID", prev_tenant);
        restore_env("MS_GRAPH_CLIENT_ID", prev_client);
        restore_env("MS_GRAPH_CLIENT_SECRET", prev_secret);
    }

    #[test]
    fn receive_parses_payload() {
        let adapter = TeamsReceiveAdapter::default();
        let ctx = make_tenant_ctx("acme".into(), None, None);
        let payload = json!({
            "id": "msg-1",
            "body": { "content": "Hello" },
            "channelIdentity": { "teamId": "team-1", "channelId": "channel-1" },
            "replyToId": "thread-1",
            "from": { "user": { "id": "user-1" } }
        });

        let messages = adapter.ingest(&ctx, payload).unwrap();
        assert_eq!(messages.len(), 1);
        let msg = &messages[0];
        assert_eq!(msg.chat_id.as_deref(), Some("channel-1"));
        assert_eq!(msg.thread_id.as_deref(), Some("thread-1"));
        assert_eq!(msg.user_id.as_deref(), Some("user-1"));
        assert_eq!(msg.text.as_deref(), Some("Hello"));
    }
}
