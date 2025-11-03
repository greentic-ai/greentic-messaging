use super::creds::WhatsAppCredentials;
use crate::prelude::*;
use crate::secrets_paths::whatsapp_credentials;
use serde_json::json;

const GRAPH_VERSION: &str = "v19.0";

pub async fn ensure_subscription(
    client: &reqwest::Client,
    ctx: &TenantCtx,
    target_url: &str,
    api_base: &str,
    secrets: &impl SecretsResolver,
) -> NodeResult<WhatsAppCredentials> {
    let path = whatsapp_credentials(ctx);
    let mut creds: WhatsAppCredentials = secrets.get_json(&path, ctx).await?.ok_or_else(|| {
        NodeError::new(
            "missing_whatsapp_creds",
            format!("No WhatsApp creds at {}", path.as_str()),
        )
    })?;

    let fingerprint = creds.fingerprint();
    let needs_update = !creds.webhook_subscribed
        || creds
            .subscription_signature
            .as_ref()
            .map(|sig| sig != &fingerprint)
            .unwrap_or(true);

    if !needs_update {
        return Ok(creds);
    }

    if api_base.starts_with("mock://") {
        creds.webhook_subscribed = true;
        creds.subscription_signature = Some(fingerprint);
        secrets.put_json(&path, ctx, &creds).await?;
        return Ok(creds);
    }

    let base = api_base.trim_end_matches('/');
    let url = format!(
        "{}/{}/{}/subscribed_apps",
        base, GRAPH_VERSION, creds.phone_id
    );
    let body = json!({
        "access_token": creds.wa_user_token,
        "webhook_url": target_url,
        "verify_token": creds.verify_token,
    });

    let response = client.post(&url).json(&body).send().await.map_err(|err| {
        NodeError::new(
            "whatsapp_http",
            format!("whatsapp subscription failed: {err}"),
        )
    })?;

    if !response.status().is_success() {
        let status = response.status();
        let msg = response
            .text()
            .await
            .unwrap_or_else(|_| "no response body".into());
        return Err(NodeError::new(
            "whatsapp_subscription_error",
            format!("status {status}: {msg}"),
        ));
    }

    creds.webhook_subscribed = true;
    creds.subscription_signature = Some(fingerprint);
    secrets.put_json(&path, ctx, &creds).await?;

    Ok(creds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::make_tenant_ctx;
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
                serde_json::from_value(json).map(Some).map_err(|err| {
                    NodeError::new("decode", "failed to decode secret").with_source(err)
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
    async fn ensure_subscription_sets_signature_and_is_idempotent() {
        unsafe {
            std::env::set_var("GREENTIC_ENV", "test");
        }
        let ctx = make_tenant_ctx("acme".into(), None, None);
        let resolver = InMemorySecrets::default();
        let path = whatsapp_credentials(&ctx);

        let creds = WhatsAppCredentials {
            phone_id: "12345".into(),
            wa_user_token: "token".into(),
            app_secret: "secret".into(),
            verify_token: "verify".into(),
            webhook_subscribed: false,
            subscription_signature: None,
        };
        resolver.put_json(&path, &ctx, &creds).await.unwrap();

        let client = reqwest::Client::new();
        let target = "https://example.com/ingress/whatsapp/acme";
        let updated = ensure_subscription(&client, &ctx, target, "mock://wa", &resolver)
            .await
            .unwrap();
        assert!(updated.webhook_subscribed);
        let fingerprint = updated.fingerprint();
        assert_eq!(
            updated.subscription_signature.as_deref(),
            Some(fingerprint.as_str())
        );

        let stored: WhatsAppCredentials = resolver.get_json(&path, &ctx).await.unwrap().unwrap();
        assert!(stored.webhook_subscribed);
        let stored_fingerprint = stored.fingerprint();
        assert_eq!(
            stored.subscription_signature.as_deref(),
            Some(stored_fingerprint.as_str())
        );

        // Second ensure should be a no-op since signature matches.
        ensure_subscription(&client, &ctx, target, "mock://wa", &resolver)
            .await
            .unwrap();

        let stored_again: WhatsAppCredentials =
            resolver.get_json(&path, &ctx).await.unwrap().unwrap();
        assert_eq!(
            stored_again.subscription_signature,
            stored.subscription_signature
        );

        // Simulate rotation by resetting fields and updating verify token.
        let mut rotated = stored_again.clone();
        rotated.verify_token = "verify-rotated".into();
        rotated.webhook_subscribed = false;
        rotated.subscription_signature = None;
        resolver.put_json(&path, &ctx, &rotated).await.unwrap();

        let rotated_result = ensure_subscription(&client, &ctx, target, "mock://wa", &resolver)
            .await
            .unwrap();
        assert!(rotated_result.webhook_subscribed);
        assert_eq!(
            rotated_result.subscription_signature.as_deref(),
            Some(rotated_result.fingerprint().as_str())
        );
    }
}
