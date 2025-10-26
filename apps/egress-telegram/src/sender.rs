use async_trait::async_trait;
use gsm_core::egress::{EgressSender, OutboundMessage, SendResult};
use gsm_core::platforms::telegram::creds::TelegramCreds;
use gsm_core::prelude::*;
use gsm_core::provider::ProviderKey;
use gsm_core::registry::{Provider, ProviderRegistry};
use gsm_core::secrets_paths::messaging_credentials;
use gsm_core::Platform;
use serde_json::{json, Value};
use std::sync::Arc;

#[derive(Clone)]
struct TelegramProvider {
    token: String,
}

impl Provider for TelegramProvider {}

pub struct TelegramSender<R>
where
    R: SecretsResolver + Send + Sync,
{
    http: reqwest::Client,
    secrets: Arc<R>,
    registry: ProviderRegistry<TelegramProvider>,
    api_base: String,
}

impl<R> TelegramSender<R>
where
    R: SecretsResolver + Send + Sync,
{
    pub fn new(http: reqwest::Client, secrets: Arc<R>, api_base: impl Into<String>) -> Self {
        Self {
            http,
            secrets,
            registry: ProviderRegistry::new(),
            api_base: api_base.into(),
        }
    }

    async fn token_for(&self, ctx: &TenantCtx) -> NodeResult<String> {
        let key = ProviderKey {
            platform: Platform::Telegram,
            env: ctx.env.clone(),
            tenant: ctx.tenant.clone(),
            team: ctx.team.clone(),
        };
        if let Some(provider) = self.registry.get(&key) {
            return Ok(provider.token.clone());
        }

        let path = messaging_credentials("telegram", ctx);
        let creds: Option<TelegramCreds> = self.secrets.get_json(&path, ctx).await?;
        let creds = creds.ok_or_else(|| {
            NodeError::new(
                "telegram_missing_creds",
                format!("no telegram creds at {}", path.as_str()),
            )
        })?;

        let token = creds.bot_token.clone();
        let provider = Arc::new(TelegramProvider {
            token: token.clone(),
        });
        self.registry.put(key, provider);
        Ok(token)
    }
}

#[async_trait]
impl<R> EgressSender for TelegramSender<R>
where
    R: SecretsResolver + Send + Sync,
{
    async fn send(&self, ctx: &TenantCtx, msg: OutboundMessage) -> NodeResult<SendResult> {
        let channel = msg
            .channel
            .as_deref()
            .ok_or_else(|| fail("telegram_missing_channel"))?;

        if msg.payload.is_none() && msg.text.is_none() {
            return Err(fail("telegram_missing_text"));
        }

        let token = self.token_for(ctx).await?;
        let mut payload = msg.payload.clone().unwrap_or_else(|| {
            json!({
                "text": msg.text.clone().unwrap_or_default(),
            })
        });

        ensure_chat_and_text(&mut payload, channel, msg.text.as_deref())?;
        let method = extract_method(&mut payload)?;

        if self.api_base.starts_with("mock://") {
            return Ok(SendResult {
                message_id: Some(token.clone()),
                raw: Some(json!({ "payload": payload, "token": token })),
            });
        }

        let url = build_api_url(&self.api_base, &token, &method);

        let response = self
            .http
            .post(url)
            .json(&payload)
            .send()
            .await
            .map_err(net)?;
        let status = response.status();
        let body_text = response.text().await.map_err(net)?;

        if !status.is_success() {
            let mut err = fail("telegram_send_failed");
            if status.is_server_error() {
                err = err.with_retry(Some(1_000));
            }
            let details = json!({
                "status": status.as_u16(),
                "body": body_text,
            });
            return Err(err.with_details(details));
        }

        let raw: Value = serde_json::from_str(&body_text).unwrap_or(Value::Null);
        let message_id = raw
            .get("result")
            .and_then(|r| r.get("message_id"))
            .and_then(|m| m.as_i64())
            .map(|id| id.to_string());

        Ok(SendResult {
            message_id,
            raw: Some(raw),
        })
    }
}

fn ensure_chat_and_text(payload: &mut Value, chat_id: &str, text: Option<&str>) -> NodeResult<()> {
    let obj = payload
        .as_object_mut()
        .ok_or_else(|| fail("telegram_payload_not_object"))?;
    obj.entry("chat_id".to_string())
        .or_insert_with(|| Value::String(chat_id.to_string()));
    if let Some(text) = text {
        obj.entry("text".to_string())
            .or_insert_with(|| Value::String(text.to_string()));
    }
    Ok(())
}

fn extract_method(payload: &mut Value) -> NodeResult<String> {
    let obj = payload
        .as_object_mut()
        .ok_or_else(|| fail("telegram_payload_not_object"))?;
    if let Some(method) = obj.remove("method") {
        let method = method
            .as_str()
            .ok_or_else(|| fail("telegram_method_not_string"))?;
        if method.is_empty() {
            return Err(fail("telegram_method_empty"));
        }
        Ok(method.to_string())
    } else {
        Ok("sendMessage".into())
    }
}

fn build_api_url(api_base: &str, bot_token: &str, method: &str) -> String {
    format!(
        "{}/bot{}/{}",
        api_base.trim_end_matches('/'),
        bot_token,
        method
    )
}

fn fail(code: &str) -> NodeError {
    NodeError::new(code, code.to_string())
}

fn net(err: reqwest::Error) -> NodeError {
    NodeError::new("telegram_net", err.to_string())
        .with_retry(Some(1_000))
        .with_source(err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gsm_core::make_tenant_ctx;
    use serde_json::json;
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
    async fn sends_using_tenant_specific_tokens() {
        let prev_env = std::env::var("GREENTIC_ENV").ok();
        std::env::set_var("GREENTIC_ENV", "test");
        let secrets = Arc::new(InMemorySecrets::default());
        let ctx_a = make_tenant_ctx("tenant-a".into(), None, None);
        let ctx_b = make_tenant_ctx("tenant-b".into(), Some("team-1".into()), None);

        let path_a = messaging_credentials("telegram", &ctx_a);
        secrets
            .put_json(
                &path_a,
                &ctx_a,
                &TelegramCreds {
                    bot_token: "tenant-a-token".into(),
                    webhook_secret: "secret".into(),
                    webhook_set: false,
                },
            )
            .await
            .unwrap();

        let path_b = messaging_credentials("telegram", &ctx_b);
        secrets
            .put_json(
                &path_b,
                &ctx_b,
                &TelegramCreds {
                    bot_token: "tenant-b-token".into(),
                    webhook_secret: "secret".into(),
                    webhook_set: false,
                },
            )
            .await
            .unwrap();

        let sender =
            TelegramSender::new(reqwest::Client::new(), secrets.clone(), "mock://telegram");

        let res_a = sender
            .send(
                &ctx_a,
                OutboundMessage {
                    channel: Some("chat-1".into()),
                    text: Some("hello".into()),
                    payload: None,
                },
            )
            .await
            .unwrap();

        assert_eq!(res_a.message_id.as_deref(), Some("tenant-a-token"));
        assert_eq!(res_a.raw.as_ref().unwrap()["payload"]["chat_id"], "chat-1");

        let res_b = sender
            .send(
                &ctx_b,
                OutboundMessage {
                    channel: Some("chat-2".into()),
                    text: Some("hola".into()),
                    payload: None,
                },
            )
            .await
            .unwrap();

        assert_eq!(res_b.message_id.as_deref(), Some("tenant-b-token"));
        assert_eq!(res_b.raw.as_ref().unwrap()["payload"]["chat_id"], "chat-2");

        if let Some(env) = prev_env {
            std::env::set_var("GREENTIC_ENV", env);
        } else {
            std::env::remove_var("GREENTIC_ENV");
        }
    }

    #[test]
    fn extract_method_defaults_to_send_message() {
        let mut payload = json!({"text": "hello"});
        let method = extract_method(&mut payload).unwrap();
        assert_eq!(method, "sendMessage");
        assert!(!payload.as_object().unwrap().contains_key("method"));
    }

    #[test]
    fn extract_method_reads_custom_method() {
        let mut payload = json!({"method": "sendPhoto", "photo": "abc"});
        let method = extract_method(&mut payload).unwrap();
        assert_eq!(method, "sendPhoto");
    }
}
