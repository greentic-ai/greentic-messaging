use std::sync::Arc;

use async_trait::async_trait;
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::egress::{EgressSender, OutboundMessage, SendResult};
use crate::platforms::webex::creds::WebexCreds;
use crate::prelude::*;
use crate::secrets_paths::messaging_credentials;

pub struct WebexSender<R>
where
    R: SecretsResolver + Send + Sync,
{
    http: reqwest::Client,
    secrets: Arc<R>,
    api_base: String,
}

impl<R> WebexSender<R>
where
    R: SecretsResolver + Send + Sync,
{
    pub fn new(http: reqwest::Client, secrets: Arc<R>, api_base: Option<String>) -> Self {
        let base = api_base.unwrap_or_else(|| "https://webexapis.com/v1".into());
        Self {
            http,
            secrets,
            api_base: base,
        }
    }

    async fn credentials(&self, ctx: &TenantCtx) -> NodeResult<WebexCreds> {
        let path = messaging_credentials("webex", ctx);
        let creds: Option<WebexCreds> = self.secrets.get_json(&path, ctx).await?;
        creds.ok_or_else(|| {
            NodeError::new(
                "webex_missing_creds",
                format!("missing webex creds at {}", path.as_str()),
            )
        })
    }

    fn build_url(&self, path: &str) -> String {
        format!(
            "{}/{}",
            self.api_base.trim_end_matches('/'),
            path.trim_start_matches('/')
        )
    }
}

fn ensure_payload(mut payload: Value, room_id: &str, text: Option<&str>) -> NodeResult<Value> {
    let obj = payload
        .as_object_mut()
        .ok_or_else(|| NodeError::new("webex_payload_not_object", "payload must be object"))?;
    obj.entry("roomId")
        .or_insert_with(|| Value::String(room_id.to_string()));
    if let Some(text) = text
        && !obj.contains_key("markdown")
        && !obj.contains_key("text")
    {
        obj.insert("markdown".into(), Value::String(text.to_string()));
    }
    Ok(Value::Object(obj.clone()))
}

fn fail(code: &str, message: impl Into<String>) -> NodeError {
    NodeError::new(code, message)
}

fn net(err: reqwest::Error) -> NodeError {
    NodeError::new("webex_transport", err.to_string())
        .with_retry(Some(1_000))
        .with_source(err)
}

#[async_trait]
impl<R> EgressSender for WebexSender<R>
where
    R: SecretsResolver + Send + Sync,
{
    async fn send(&self, ctx: &TenantCtx, msg: OutboundMessage) -> NodeResult<SendResult> {
        let room_id = msg
            .channel
            .as_deref()
            .ok_or_else(|| fail("webex_missing_room", "channel missing"))?;

        let creds = self.credentials(ctx).await?;

        if self.api_base.starts_with("mock://") {
            return Ok(SendResult {
                message_id: Some(creds.bot_token),
                raw: Some(json!({
                    "roomId": room_id,
                    "payload": msg.payload,
                    "text": msg.text,
                })),
            });
        }

        let payload = if let Some(body) = msg.payload.clone() {
            ensure_payload(body, room_id, msg.text.as_deref())?
        } else {
            json!({
                "roomId": room_id,
                "markdown": msg.text.unwrap_or_default(),
            })
        };

        let url = self.build_url("messages");
        let response = self
            .http
            .post(url)
            .bearer_auth(&creds.bot_token)
            .json(&payload)
            .send()
            .await
            .map_err(net)?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            let mut err = fail(
                "webex_send_failed",
                format!("status={} body={}", status.as_u16(), body),
            );
            if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
                err = err.with_retry(Some(1_000));
            }
            return Err(err.with_detail_text(
                serde_json::to_string(&json!({
                    "status": status.as_u16(),
                    "body": body,
                }))
                .unwrap_or_else(|_| "{\"error\":\"failed to encode details\"}".to_string()),
            ));
        }

        let raw: Value = response.json().await.unwrap_or(Value::Null);
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
    use crate::{current_env, set_current_env};
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
    async fn loads_token_per_tenant() {
        let prev_env = current_env();
        set_current_env(EnvId::try_from("test").expect("valid env id"));

        let secrets = Arc::new(InMemorySecrets::default());
        let ctx_a = make_tenant_ctx("tenant-a".into(), None, None);
        let ctx_b = make_tenant_ctx("tenant-b".into(), Some("team-1".into()), None);

        let path_a = messaging_credentials("webex", &ctx_a);
        secrets
            .put_json(
                &path_a,
                &ctx_a,
                &WebexCreds {
                    bot_token: "token-a".into(),
                },
            )
            .await
            .unwrap();

        let path_b = messaging_credentials("webex", &ctx_b);
        secrets
            .put_json(
                &path_b,
                &ctx_b,
                &WebexCreds {
                    bot_token: "token-b".into(),
                },
            )
            .await
            .unwrap();

        let sender = WebexSender::new(reqwest::Client::new(), secrets, Some("mock://webex".into()));

        let res_a = sender
            .send(
                &ctx_a,
                OutboundMessage {
                    channel: Some("room-1".into()),
                    text: Some("hello".into()),
                    payload: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(res_a.message_id.as_deref(), Some("token-a"));

        let res_b = sender
            .send(
                &ctx_b,
                OutboundMessage {
                    channel: Some("room-2".into()),
                    text: Some("hola".into()),
                    payload: Some(json!({"roomId": "room-2"})),
                },
            )
            .await
            .unwrap();
        assert_eq!(res_b.message_id.as_deref(), Some("token-b"));

        set_current_env(prev_env);
    }

    #[tokio::test]
    async fn requires_channel() {
        let prev_env = current_env();
        set_current_env(EnvId::try_from("test").expect("valid env id"));
        let secrets = Arc::new(InMemorySecrets::default());
        let sender = WebexSender::new(reqwest::Client::new(), secrets, Some("mock://webex".into()));
        let ctx = make_tenant_ctx("acme".into(), None, None);
        let err = sender
            .send(
                &ctx,
                OutboundMessage {
                    channel: None,
                    text: Some("hi".into()),
                    payload: None,
                },
            )
            .await
            .expect_err("missing room");
        assert_eq!(err.to_string(), "webex_missing_room: channel missing");
        set_current_env(prev_env);
    }

    #[test]
    fn webex_secrets_and_urls_are_scoped() {
        let ctx_a = make_tenant_ctx("acme".into(), Some("support".into()), None);
        let ctx_b = make_tenant_ctx("globex".into(), Some("sales".into()), None);
        let path_a = messaging_credentials("webex", &ctx_a);
        let path_b = messaging_credentials("webex", &ctx_b);
        assert!(path_a.as_str().contains("/acme/"));
        assert!(path_b.as_str().contains("/globex/"));
        assert_ne!(path_a.as_str(), path_b.as_str());

        let secrets = Arc::new(InMemorySecrets::default());
        let sender_default = WebexSender::new(
            reqwest::Client::new(),
            secrets.clone(),
            Some("https://webexapis.com/v1".into()),
        );
        let sender_alt = WebexSender::new(
            reqwest::Client::new(),
            secrets,
            Some("https://gov.webexapis.us/v1".into()),
        );

        assert_eq!(
            sender_default.build_url("messages"),
            "https://webexapis.com/v1/messages"
        );
        assert_eq!(
            sender_alt.build_url("messages"),
            "https://gov.webexapis.us/v1/messages"
        );
    }
}
