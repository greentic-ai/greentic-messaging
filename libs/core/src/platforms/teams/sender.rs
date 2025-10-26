use std::sync::Arc;

use async_trait::async_trait;
use reqwest::StatusCode;
use serde_json::{json, Value};

use crate::egress::{EgressSender, OutboundMessage, SendResult};
use crate::platforms::teams::conversations::{TeamsConversation, TeamsConversations};
use crate::prelude::*;
use crate::secrets_paths::{messaging_credentials, teams_conversations_secret};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TeamsCredentials {
    pub tenant_id: String,
    pub client_id: String,
    pub client_secret: String,
}

pub struct TeamsSender<R>
where
    R: SecretsResolver + Send + Sync,
{
    http: reqwest::Client,
    secrets: Arc<R>,
    auth_base: String,
    api_base: String,
}

fn normalise_api_base(api_base: Option<String>) -> String {
    match api_base {
        Some(base) => {
            let trimmed = base.trim_end_matches('/');
            if trimmed.ends_with("/v1.0") {
                trimmed.to_string()
            } else {
                format!("{}/v1.0", trimmed)
            }
        }
        None => "https://graph.microsoft.com/v1.0".into(),
    }
}

impl<R> TeamsSender<R>
where
    R: SecretsResolver + Send + Sync,
{
    pub fn new(
        http: reqwest::Client,
        secrets: Arc<R>,
        auth_base: Option<String>,
        api_base: Option<String>,
    ) -> Self {
        Self {
            http,
            secrets,
            auth_base: auth_base.unwrap_or_else(|| "https://login.microsoftonline.com".into()),
            api_base: normalise_api_base(api_base),
        }
    }

    async fn credentials(&self, ctx: &TenantCtx) -> NodeResult<TeamsCredentials> {
        let path = messaging_credentials("teams", ctx);
        let creds: Option<TeamsCredentials> = self.secrets.get_json(&path, ctx).await?;
        creds.ok_or_else(|| {
            self.fail(
                "teams_missing_creds",
                format!("missing teams credentials at {}", path.as_str()),
            )
        })
    }

    async fn conversation(&self, ctx: &TenantCtx, channel: &str) -> NodeResult<TeamsConversation> {
        let path = teams_conversations_secret(ctx);
        let store: Option<TeamsConversations> = self.secrets.get_json(&path, ctx).await?;
        if let Some(store) = store {
            if let Some(conv) = store.get(channel) {
                return Ok(conv.clone());
            }
        }
        Err(self.fail(
            "teams_missing_conversation",
            format!(
                "no conversation for channel '{}' under {}",
                channel,
                path.as_str()
            ),
        ))
    }

    fn token_url(&self, tenant_id: &str) -> String {
        let base = self.auth_base.trim_end_matches('/');
        format!("{base}/{tenant_id}/oauth2/v2.0/token")
    }

    fn messages_url(&self, chat_id: &str) -> String {
        let base = self.api_base.trim_end_matches('/');
        format!("{base}/chats/{chat_id}/messages")
    }

    async fn token(&self, creds: &TeamsCredentials) -> NodeResult<String> {
        if self.auth_base.starts_with("mock://") {
            return Ok("mock-token".into());
        }

        let url = self.token_url(&creds.tenant_id);
        let form = [
            ("client_id", creds.client_id.as_str()),
            ("client_secret", creds.client_secret.as_str()),
            ("grant_type", "client_credentials"),
            ("scope", "https://graph.microsoft.com/.default"),
        ];

        let response = self
            .http
            .post(url)
            .form(&form)
            .send()
            .await
            .map_err(|err| self.net(err))?;

        let status = response.status();
        let body = response.text().await.map_err(|err| self.net(err))?;
        if !status.is_success() {
            let mut err = self.fail(
                "teams_token_failed",
                format!("status={} body={}", status.as_u16(), body),
            );
            if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
                err = err.with_retry(Some(1_000));
            }
            return Err(err.with_details(json!({
                "status": status.as_u16(),
                "body": body,
            })));
        }

        let value: Value = serde_json::from_str(&body)
            .map_err(|err| self.fail("teams_token_decode", err.to_string()))?;
        value
            .get("access_token")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| self.fail("teams_token_missing", "access_token missing in response"))
    }

    fn build_card_payload(payload: &Value) -> Value {
        json!({
          "subject": Value::Null,
          "importance": "normal",
          "body": { "contentType": "html", "content": " " },
          "attachments": [{
            "id": "1",
            "contentType": "application/vnd.microsoft.card.adaptive",
            "contentUrl": Value::Null,
            "content": payload,
            "name": "card.json",
            "thumbnailUrl": Value::Null
          }]
        })
    }

    fn build_text_payload(text: &str) -> Value {
        json!({
          "body": {
            "contentType": "text",
            "content": text
          }
        })
    }

    fn fail(&self, code: &str, message: impl Into<String>) -> NodeError {
        NodeError::new(code, message)
    }

    fn net(&self, err: reqwest::Error) -> NodeError {
        NodeError::new("teams_transport", err.to_string())
            .with_retry(Some(1_000))
            .with_source(err)
    }
}

#[async_trait]
impl<R> EgressSender for TeamsSender<R>
where
    R: SecretsResolver + Send + Sync,
{
    async fn send(&self, ctx: &TenantCtx, msg: OutboundMessage) -> NodeResult<SendResult> {
        let channel = msg
            .channel
            .as_deref()
            .ok_or_else(|| self.fail("teams_missing_channel", "channel missing"))?;
        let conversation = self.conversation(ctx, channel).await?;
        let creds = self.credentials(ctx).await?;

        if self.api_base.starts_with("mock://") {
            return Ok(SendResult {
                message_id: Some(format!("mock:{}", conversation.chat_id)),
                raw: Some(json!({
                    "chat_id": conversation.chat_id,
                    "payload": msg.payload,
                    "text": msg.text,
                })),
            });
        }

        let token = self.token(&creds).await?;
        let body = if let Some(payload) = msg.payload.as_ref() {
            Self::build_card_payload(payload)
        } else {
            let text = msg.text.as_deref().unwrap_or_default();
            Self::build_text_payload(text)
        };

        let url = self.messages_url(&conversation.chat_id);
        let response = self
            .http
            .post(url)
            .bearer_auth(&token)
            .json(&body)
            .send()
            .await
            .map_err(|err| self.net(err))?;

        let status = response.status();
        let retryable = status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error();
        let body_text = response.text().await.map_err(|err| self.net(err))?;

        if !status.is_success() {
            let mut err = self.fail(
                "teams_send_failed",
                format!("status={} body={}", status.as_u16(), body_text),
            );
            if retryable {
                err = err.with_retry(Some(1_000));
            }
            return Err(err.with_details(json!({
                "status": status.as_u16(),
                "body": body_text,
            })));
        }

        let raw: Value = serde_json::from_str(&body_text).unwrap_or(Value::Null);
        let message_id = raw
            .get("id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        Ok(SendResult {
            message_id,
            raw: Some(raw),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::make_tenant_ctx;
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[derive(Default)]
    struct InMemorySecrets {
        store: Mutex<HashMap<String, Value>>,
    }

    #[async_trait]
    impl SecretsResolver for InMemorySecrets {
        async fn get_json<T>(&self, path: &SecretPath, _ctx: &TenantCtx) -> NodeResult<Option<T>>
        where
            T: serde::de::DeserializeOwned + Send,
        {
            let value = self.store.lock().unwrap().get(path.as_str()).cloned();
            if let Some(json) = value {
                Ok(Some(serde_json::from_value(json).map_err(|err| {
                    NodeError::new("decode", "failed to decode secret").with_source(err)
                })?))
            } else {
                Ok(None)
            }
        }

        async fn put_json<T>(
            &self,
            path: &SecretPath,
            _ctx: &TenantCtx,
            value: &T,
        ) -> NodeResult<()>
        where
            T: serde::Serialize + Sync + Send,
        {
            let json = serde_json::to_value(value).map_err(|err| {
                NodeError::new("encode", "failed to encode secret").with_source(err)
            })?;
            self.store
                .lock()
                .unwrap()
                .insert(path.as_str().to_string(), json);
            Ok(())
        }
    }

    #[tokio::test]
    async fn loads_credentials_and_conversation() {
        let prev_env = std::env::var("GREENTIC_ENV").ok();
        std::env::set_var("GREENTIC_ENV", "test");

        let secrets = Arc::new(InMemorySecrets::default());
        let ctx = make_tenant_ctx("tenant-a".into(), Some("team-a".into()), None);
        let creds_path = messaging_credentials("teams", &ctx);
        secrets
            .put_json(
                &creds_path,
                &ctx,
                &TeamsCredentials {
                    tenant_id: "t".into(),
                    client_id: "id".into(),
                    client_secret: "secret".into(),
                },
            )
            .await
            .unwrap();
        let conv_path = teams_conversations_secret(&ctx);
        let mut store = TeamsConversations::default();
        store.insert("channel-1", TeamsConversation::new("chat-123"));
        secrets.put_json(&conv_path, &ctx, &store).await.unwrap();

        let sender = TeamsSender::new(
            reqwest::Client::new(),
            secrets,
            Some("mock://auth".into()),
            Some("mock://graph".into()),
        );
        let res = sender
            .send(
                &ctx,
                OutboundMessage {
                    channel: Some("channel-1".into()),
                    text: Some("hello".into()),
                    payload: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(res.message_id.as_deref(), Some("mock:chat-123"));

        if let Some(env) = prev_env {
            std::env::set_var("GREENTIC_ENV", env);
        } else {
            std::env::remove_var("GREENTIC_ENV");
        }
    }

    #[tokio::test]
    async fn missing_conversation_errors() {
        let prev_env = std::env::var("GREENTIC_ENV").ok();
        std::env::set_var("GREENTIC_ENV", "test");

        let secrets = Arc::new(InMemorySecrets::default());
        let ctx = make_tenant_ctx("tenant-a".into(), Some("team-a".into()), None);
        let creds_path = messaging_credentials("teams", &ctx);
        secrets
            .put_json(
                &creds_path,
                &ctx,
                &TeamsCredentials {
                    tenant_id: "t".into(),
                    client_id: "id".into(),
                    client_secret: "secret".into(),
                },
            )
            .await
            .unwrap();

        let sender = TeamsSender::new(
            reqwest::Client::new(),
            secrets,
            Some("mock://auth".into()),
            Some("mock://graph".into()),
        );
        let err = sender
            .send(
                &ctx,
                OutboundMessage {
                    channel: Some("missing".into()),
                    text: Some("hello".into()),
                    payload: None,
                },
            )
            .await
            .expect_err("missing conversation");
        assert_eq!(
            err.to_string(),
            "teams_missing_conversation: no conversation for channel 'missing' under /test/messaging/teams/tenant-a/team-a/conversations.json"
        );

        if let Some(env) = prev_env {
            std::env::set_var("GREENTIC_ENV", env);
        } else {
            std::env::remove_var("GREENTIC_ENV");
        }
    }

    #[test]
    fn credentials_and_urls_scope_by_context() {
        let ctx_a = make_tenant_ctx("acme".into(), Some("support".into()), None);
        let ctx_b = make_tenant_ctx("globex".into(), Some("sales".into()), None);
        let path_a = messaging_credentials("teams", &ctx_a);
        let path_b = messaging_credentials("teams", &ctx_b);
        assert!(path_a.as_str().contains("/acme/"));
        assert!(path_b.as_str().contains("/globex/"));
        assert_ne!(path_a.as_str(), path_b.as_str());

        let secrets = Arc::new(InMemorySecrets::default());
        let sender_default = TeamsSender::new(
            reqwest::Client::new(),
            secrets.clone(),
            Some("https://login.microsoftonline.com".into()),
            Some("https://graph.microsoft.com/v1.0".into()),
        );
        let sender_alt = TeamsSender::new(
            reqwest::Client::new(),
            secrets,
            Some("https://login.partner.microsoftonline.cn".into()),
            Some("https://microsoftgraph.chinacloudapi.cn/v1.0".into()),
        );

        assert_eq!(
            sender_default.messages_url("19:abc"),
            "https://graph.microsoft.com/v1.0/chats/19:abc/messages"
        );
        assert_eq!(
            sender_alt.messages_url("19:abc"),
            "https://microsoftgraph.chinacloudapi.cn/v1.0/chats/19:abc/messages"
        );
    }
}
