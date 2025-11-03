use super::creds::{WebexCredentials, WebexWebhook};
use crate::prelude::*;
use crate::secrets_paths::webex_credentials;
use serde::Deserialize;
use serde_json::json;

const REQUIRED_WEBHOOKS: &[(&str, &str)] = &[("messages", "created"), ("memberships", "created")];

#[derive(Debug, Deserialize)]
struct WebexWebhookResponse {
    id: String,
    resource: String,
    event: String,
}

pub async fn ensure_webhooks(
    client: &reqwest::Client,
    ctx: &TenantCtx,
    target_url: &str,
    api_base: &str,
    secrets: &impl SecretsResolver,
) -> NodeResult<WebexCredentials> {
    let path = webex_credentials(ctx);
    let mut creds: WebexCredentials = secrets.get_json(&path, ctx).await?.ok_or_else(|| {
        NodeError::new(
            "missing_webex_creds",
            format!("No Webex creds at {}", path.as_str()),
        )
    })?;

    let mut updated = false;
    for (resource, event) in REQUIRED_WEBHOOKS {
        if creds.has_subscription(resource, event) {
            continue;
        }
        let hook = create_webhook(client, &creds, api_base, target_url, resource, event).await?;
        creds.webhooks.push(hook);
        updated = true;
    }

    if updated {
        secrets.put_json(&path, ctx, &creds).await?;
    }

    Ok(creds)
}

async fn create_webhook(
    client: &reqwest::Client,
    creds: &WebexCredentials,
    api_base: &str,
    target_url: &str,
    resource: &str,
    event: &str,
) -> NodeResult<WebexWebhook> {
    if api_base.starts_with("mock://") {
        return Ok(WebexWebhook {
            id: format!("mock-{}-{}", resource, event),
            resource: resource.to_string(),
            event: event.to_string(),
        });
    }

    let url = format!("{}/webhooks", api_base.trim_end_matches('/'));
    let body = json!({
        "name": format!("greentic-{}-{}", resource, event),
        "targetUrl": target_url,
        "resource": resource,
        "event": event,
        "secret": creds.webhook_secret,
    });

    let response = client
        .post(&url)
        .bearer_auth(&creds.bot_token)
        .json(&body)
        .send()
        .await
        .map_err(|err| {
            NodeError::new("webex_http", format!("webex webhook request failed: {err}"))
        })?
        .error_for_status()
        .map_err(|err| NodeError::new("webex_http", format!("webex webhook error: {err}")))?;

    let webhook: WebexWebhookResponse = response.json().await.map_err(|err| {
        NodeError::new(
            "webex_decode",
            format!("failed to decode webhook response: {err}"),
        )
    })?;

    Ok(WebexWebhook {
        id: webhook.id,
        resource: webhook.resource,
        event: webhook.event,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{make_tenant_ctx, webex_credentials};
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[derive(Default)]
    struct InMemorySecrets {
        store: Mutex<HashMap<String, serde_json::Value>>,
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
                    NodeError::new("decode", format!("failed to decode secret: {err}"))
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
                NodeError::new("encode", format!("failed to encode secret: {err}"))
            })?;
            self.store
                .lock()
                .unwrap()
                .insert(path.as_str().to_string(), json);
            Ok(())
        }
    }

    #[tokio::test]
    async fn ensure_webhooks_posts_and_persists_ids() {
        let prev_env = std::env::var("GREENTIC_ENV").ok();
        unsafe {
            std::env::set_var("GREENTIC_ENV", "test");
        }
        let ctx = make_tenant_ctx("acme".into(), Some("default".into()), None);
        let resolver = InMemorySecrets::default();
        let path = webex_credentials(&ctx);
        let creds = WebexCredentials {
            bot_token: "TOKEN".into(),
            webhook_secret: "secret".into(),
            webhooks: Vec::new(),
        };
        resolver.put_json(&path, &ctx, &creds).await.unwrap();

        let client = reqwest::Client::new();
        let target_url = "https://example.com/ingress/webex/acme/default";
        let api_base = "mock://webex";
        let updated = ensure_webhooks(&client, &ctx, target_url, api_base, &resolver)
            .await
            .expect("webhooks");

        assert_eq!(updated.webhooks.len(), 2);
        assert!(updated.has_subscription("messages", "created"));
        assert!(updated.has_subscription("memberships", "created"));

        // Subsequent call should be idempotent and keep the same number of webhooks
        let again = ensure_webhooks(&client, &ctx, target_url, api_base, &resolver)
            .await
            .expect("webhooks");
        assert_eq!(again.webhooks.len(), 2);

        if let Some(env) = prev_env {
            unsafe {
                std::env::set_var("GREENTIC_ENV", env);
            }
        } else {
            unsafe {
                std::env::remove_var("GREENTIC_ENV");
            }
        }
    }
}
