use super::creds::TelegramCreds;
use crate::prelude::*;
use crate::secrets_paths::messaging_credentials;

pub async fn ensure_provisioned(
    client: &reqwest::Client,
    ctx: &TenantCtx,
    base_url: &str,
    api_base: &str,
    secrets: &impl SecretsResolver,
) -> NodeResult<()> {
    let path = messaging_credentials("telegram", ctx);
    let mut creds: TelegramCreds = secrets.get_json(&path, ctx).await?.ok_or_else(|| {
        NodeError::new(
            "missing_creds",
            format!("No Telegram creds at {}", path.as_str()),
        )
    })?;

    if !creds.webhook_set {
        let team = ctx.team.as_ref().map(|t| t.0.as_str()).unwrap_or("default");
        let base = base_url.trim_end_matches('/');
        let webhook_url = format!(
            "{}/ingress/telegram/{}/{}/{}",
            base, ctx.tenant.0, team, creds.webhook_secret
        );
        let api_root = api_base.trim_end_matches('/');
        let api = format!("{}/bot{}/setWebhook", api_root, creds.bot_token);
        let body = serde_json::json!({ "url": webhook_url });
        #[cfg(test)]
        {
            if api.starts_with("mock://") {
                creds.webhook_set = true;
                secrets.put_json(&path, ctx, &creds).await?;
                return Ok(());
            }
        }
        let response = client.post(&api).json(&body).send().await.map_err(|err| {
            NodeError::new("net", "failed to call Telegram setWebhook")
                .with_source(err)
                .with_retry(Some(1_000))
        })?;
        if response.status().is_success() {
            creds.webhook_set = true;
            secrets.put_json(&path, ctx, &creds).await?;
        } else {
            let msg = response.text().await.unwrap_or_default();
            return Err(NodeError::new("telegram_set_webhook_failed", msg).with_retry(Some(5_000)));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::make_tenant_ctx;
    use async_trait::async_trait;
    use serde::de::DeserializeOwned;
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
            T: DeserializeOwned + Send,
        {
            let value = self.store.lock().unwrap().get(path.as_str()).cloned();
            if let Some(json) = value {
                serde_json::from_value(json).map(Some).map_err(|err| {
                    NodeError::new("secrets_decode", "failed to decode secret").with_source(err)
                })
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
                NodeError::new("secrets_encode", "failed to encode secret").with_source(err)
            })?;
            self.store
                .lock()
                .unwrap()
                .insert(path.as_str().to_string(), json);
            Ok(())
        }
    }

    #[tokio::test]
    async fn ensure_provisioned_updates_secret() {
        std::env::set_var("GREENTIC_ENV", "test");
        let ctx = make_tenant_ctx("acme".into(), Some("team-1".into()), None);

        let creds = TelegramCreds {
            bot_token: "TOKEN".into(),
            webhook_secret: "secret".into(),
            webhook_set: false,
        };

        let resolver = InMemorySecrets::default();
        let path = messaging_credentials("telegram", &ctx);
        resolver.put_json(&path, &ctx, &creds).await.unwrap();

        let client = reqwest::Client::new();
        ensure_provisioned(
            &client,
            &ctx,
            "https://public.example",
            "mock://telegram.api",
            &resolver,
        )
        .await
        .unwrap();

        let stored: TelegramCreds = resolver.get_json(&path, &ctx).await.unwrap().unwrap();
        assert!(stored.webhook_set, "webhook flag should be persisted");
    }
}
