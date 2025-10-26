use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::egress::{EgressSender, OutboundMessage, SendResult};
use crate::prelude::*;
use crate::secrets_paths::messaging_credentials;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhatsAppCreds {
    pub phone_id: String,
    #[serde(alias = "wa_user_token")]
    pub user_token: String,
}

pub struct WhatsAppSender<R>
where
    R: SecretsResolver + Send + Sync,
{
    http: reqwest::Client,
    secrets: Arc<R>,
    api_base: String,
}

impl<R> WhatsAppSender<R>
where
    R: SecretsResolver + Send + Sync,
{
    pub fn new(http: reqwest::Client, secrets: Arc<R>, api_base: Option<String>) -> Self {
        let base = api_base.unwrap_or_else(|| "https://graph.facebook.com/v19.0".into());
        Self {
            http,
            secrets,
            api_base: base.trim_end_matches('/').to_string(),
        }
    }

    pub async fn credentials(&self, ctx: &TenantCtx) -> NodeResult<WhatsAppCreds> {
        let path = messaging_credentials("whatsapp", ctx);
        let creds: Option<WhatsAppCreds> = self.secrets.get_json(&path, ctx).await?;
        creds.ok_or_else(|| {
            self.fail(
                "wa_missing_creds",
                format!("missing whatsapp creds at {}", path.as_str()),
            )
        })
    }

    pub fn api_base(&self) -> &str {
        &self.api_base
    }

    fn fail(&self, code: &str, message: impl Into<String>) -> NodeError {
        NodeError::new(code, message)
    }

    fn net(&self, err: reqwest::Error) -> NodeError {
        NodeError::new("wa_transport", err.to_string())
            .with_retry(Some(1_000))
            .with_source(err)
    }

    fn build_url(&self, phone_id: &str) -> String {
        format!("{}/{}/messages", self.api_base, phone_id)
    }
}

#[async_trait]
impl<R> EgressSender for WhatsAppSender<R>
where
    R: SecretsResolver + Send + Sync,
{
    async fn send(&self, ctx: &TenantCtx, msg: OutboundMessage) -> NodeResult<SendResult> {
        let creds = self.credentials(ctx).await?;
        let to = msg
            .channel
            .as_deref()
            .ok_or_else(|| self.fail("wa_missing_to", "missing whatsapp channel"))?;

        let text = msg.text.unwrap_or_default();
        let payload = serde_json::json!({
            "messaging_product": "whatsapp",
            "to": to,
            "type": "text",
            "text": {
                "body": text
            }
        });

        if self.api_base.starts_with("mock://") {
            return Ok(SendResult {
                message_id: Some(format!("mock:{}", creds.phone_id)),
                raw: Some(payload),
            });
        }

        let url = self.build_url(&creds.phone_id);
        let response = self
            .http
            .post(url)
            .bearer_auth(&creds.user_token)
            .json(&payload)
            .send()
            .await
            .map_err(|err| self.net(err))?;

        let status = response.status();
        if !status.is_success() {
            let body_text = response.text().await.unwrap_or_default();
            return Err(self
                .fail(
                    "wa_send_failed",
                    format!("status={} body={}", status.as_u16(), body_text),
                )
                .with_details(serde_json::json!({
                    "status": status.as_u16(),
                    "body": body_text,
                })));
        }

        let raw: Value = response.json().await.unwrap_or(Value::Null);
        let message_id = raw
            .get("messages")
            .and_then(|v| v.get(0))
            .and_then(|v| v.get("id"))
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
                serde_json::from_value(json)
                    .map(Some)
                    .map_err(|err| NodeError::new("decode", err.to_string()))
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
            let json = serde_json::to_value(value)
                .map_err(|err| NodeError::new("encode", err.to_string()))?;
            self.store
                .lock()
                .unwrap()
                .insert(path.as_str().to_string(), json);
            Ok(())
        }
    }

    #[tokio::test]
    async fn loads_credentials_and_returns_mock() {
        let prev_env = std::env::var("GREENTIC_ENV").ok();
        std::env::set_var("GREENTIC_ENV", "test");
        let ctx = make_tenant_ctx("acme".into(), Some("team1".into()), None);
        let secrets = Arc::new(InMemorySecrets::default());
        let path = messaging_credentials("whatsapp", &ctx);
        secrets
            .put_json(
                &path,
                &ctx,
                &serde_json::json!({
                    "phone_id": "123",
                    "wa_user_token": "token",
                }),
            )
            .await
            .unwrap();

        let sender = WhatsAppSender::new(reqwest::Client::new(), secrets, Some("mock://wa".into()));
        let res = sender
            .send(
                &ctx,
                OutboundMessage {
                    channel: Some("441100111222".into()),
                    text: Some("hello".into()),
                    payload: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(res.message_id.as_deref(), Some("mock:123"));

        if let Some(env) = prev_env {
            std::env::set_var("GREENTIC_ENV", env);
        } else {
            std::env::remove_var("GREENTIC_ENV");
        }
    }

    #[tokio::test]
    async fn requires_channel() {
        let secrets = Arc::new(InMemorySecrets::default());
        let ctx = make_tenant_ctx("acme".into(), None, None);
        let path = messaging_credentials("whatsapp", &ctx);
        secrets
            .put_json(
                &path,
                &ctx,
                &serde_json::json!({
                    "phone_id": "123",
                    "user_token": "token",
                }),
            )
            .await
            .unwrap();
        let sender = WhatsAppSender::new(reqwest::Client::new(), secrets, Some("mock://wa".into()));
        let err = sender
            .send(
                &ctx,
                OutboundMessage {
                    channel: None,
                    text: Some("hello".into()),
                    payload: None,
                },
            )
            .await
            .expect_err("missing channel");
        assert_eq!(err.to_string(), "wa_missing_to: missing whatsapp channel");
    }

    #[test]
    fn whatsapp_credentials_and_urls_are_scoped() {
        let ctx_a = make_tenant_ctx("acme".into(), Some("support".into()), None);
        let ctx_b = make_tenant_ctx("globex".into(), Some("sales".into()), None);
        let path_a = messaging_credentials("whatsapp", &ctx_a);
        let path_b = messaging_credentials("whatsapp", &ctx_b);
        assert!(path_a.as_str().contains("/acme/"));
        assert!(path_b.as_str().contains("/globex/"));
        assert_ne!(path_a.as_str(), path_b.as_str());

        let secrets = Arc::new(InMemorySecrets::default());
        let sender_default = WhatsAppSender::new(
            reqwest::Client::new(),
            secrets.clone(),
            Some("https://graph.facebook.com/v19.0".into()),
        );
        let sender_alt = WhatsAppSender::new(
            reqwest::Client::new(),
            secrets,
            Some("https://graph.facebook.com/v18.0".into()),
        );

        assert_eq!(
            sender_default.build_url("123"),
            "https://graph.facebook.com/v19.0/123/messages"
        );
        assert_eq!(
            sender_alt.build_url("456"),
            "https://graph.facebook.com/v18.0/456/messages"
        );
    }
}
