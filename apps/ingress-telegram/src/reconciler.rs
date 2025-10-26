use crate::{
    config::{TelegramConfig, Tenant},
    secrets::SecretsManager,
    telegram_api::{TelegramApi, WebhookInfo},
};
use anyhow::{Context, Result};
use gsm_telemetry::{record_counter, TelemetryLabels};
use rand::{rng, Rng};
use tracing::{info, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconcileResult {
    Applied,
    Noop,
    Error,
}

#[derive(Debug, Clone)]
pub struct TenantOutcome {
    #[allow(dead_code)]
    pub tenant: String,
    #[allow(dead_code)]
    pub secret: Option<String>,
    #[allow(dead_code)]
    pub result: ReconcileResult,
}

pub async fn reconcile_all_telegram_webhooks<TApi, TSecrets>(
    tenants: &[Tenant],
    secrets: &TSecrets,
    api: &TApi,
) -> Vec<TenantOutcome>
where
    TApi: TelegramApi + Sync + ?Sized,
    TSecrets: SecretsManager + Sync + ?Sized,
{
    let mut outcomes = Vec::new();
    for tenant in tenants {
        match tenant.telegram.as_ref() {
            Some(cfg) if cfg.enabled => {
                let outcome = reconcile_tenant(api, secrets, tenant, cfg).await;
                outcomes.push(outcome);
            }
            Some(_) => {
                info!(
                    tenant = %tenant.id,
                    event = "telegram_webhook_reconcile",
                    action = "skipped_disabled",
                    "telegram config disabled; skipping reconcile"
                );
            }
            None => {
                info!(
                    tenant = %tenant.id,
                    event = "telegram_webhook_reconcile",
                    action = "skipped_missing",
                    "no telegram config found; skipping reconcile"
                );
            }
        }
    }
    outcomes
}

fn record_metric(tenant: &str, result: ReconcileResult) {
    let result_label = match result {
        ReconcileResult::Applied => "applied",
        ReconcileResult::Noop => "noop",
        ReconcileResult::Error => "error",
    };
    let labels = TelemetryLabels {
        tenant: tenant.to_string(),
        platform: Some("telegram".into()),
        chat_id: None,
        msg_id: None,
        extra: vec![("result".into(), result_label.into())],
    };
    record_counter("greentic_telegram_webhook_reconciles_total", 1, &labels);
}

async fn reconcile_tenant<TApi, TSecrets>(
    api: &TApi,
    secrets: &TSecrets,
    tenant: &Tenant,
    cfg: &TelegramConfig,
) -> TenantOutcome
where
    TApi: TelegramApi + Sync + ?Sized,
    TSecrets: SecretsManager + Sync + ?Sized,
{
    match reconcile_tenant_inner(api, secrets, tenant, cfg).await {
        Ok((secret, result, info)) => {
            let action = match result {
                ReconcileResult::Applied => "applied",
                ReconcileResult::Noop => "noop",
                ReconcileResult::Error => "error",
            };
            info!(
                event = "telegram_webhook_reconcile",
                tenant = %tenant.id,
                action = action,
                want_url = %info.want_url,
                current_url = %info.current_url.unwrap_or_default(),
                drop_pending = info.drop_pending,
            );
            record_metric(&tenant.id, result);
            TenantOutcome {
                tenant: tenant.id.clone(),
                secret: Some(secret),
                result,
            }
        }
        Err(err) => {
            warn!(
                event = "telegram_webhook_reconcile",
                tenant = %tenant.id,
                action = "error",
                error = %err
            );
            record_metric(&tenant.id, ReconcileResult::Error);
            TenantOutcome {
                tenant: tenant.id.clone(),
                secret: None,
                result: ReconcileResult::Error,
            }
        }
    }
}

struct TenantReconcileInfo {
    want_url: String,
    current_url: Option<String>,
    drop_pending: bool,
}

async fn reconcile_tenant_inner<TApi, TSecrets>(
    api: &TApi,
    secrets: &TSecrets,
    tenant: &Tenant,
    cfg: &TelegramConfig,
) -> Result<(String, ReconcileResult, TenantReconcileInfo)>
where
    TApi: TelegramApi + Sync + ?Sized,
    TSecrets: SecretsManager + Sync + ?Sized,
{
    let bot_token_key = format!("tenants/{}/telegram/bot_token", tenant.id);
    let bot_token = secrets
        .get(&bot_token_key)
        .await?
        .with_context(|| format!("missing bot token for {}", tenant.id))?;

    let secret_token = ensure_secret(secrets, tenant.id.as_str(), cfg).await?;

    let info = api
        .get_webhook_info(&bot_token)
        .await
        .with_context(|| format!("get webhook info for {}", tenant.id))?;
    let want_url = desired_webhook_url(cfg, &tenant.id);

    let (result, drop_pending, current_url) =
        evaluate_webhook(api, info, &bot_token, &want_url, &secret_token, cfg).await?;

    Ok((
        secret_token,
        result,
        TenantReconcileInfo {
            want_url,
            current_url,
            drop_pending,
        },
    ))
}

async fn evaluate_webhook<TApi>(
    api: &TApi,
    info: WebhookInfo,
    bot_token: &str,
    want_url: &str,
    secret: &str,
    cfg: &TelegramConfig,
) -> Result<(ReconcileResult, bool, Option<String>)>
where
    TApi: TelegramApi + Sync + ?Sized,
{
    let current_url = info.url.clone();
    if urls_match(&info.url, want_url) && !info.url.trim().is_empty() {
        return Ok((ReconcileResult::Noop, false, Some(current_url)));
    }

    let first_install = info.url.is_empty();
    let drop_pending = if first_install {
        cfg.drop_pending_on_first_install()
    } else {
        false
    };
    let allowed = allowed_updates(cfg);
    api.set_webhook(bot_token, want_url, secret, &allowed, drop_pending)
        .await
        .with_context(|| format!("set webhook for {}", want_url))?;
    Ok((ReconcileResult::Applied, drop_pending, Some(current_url)))
}

pub async fn ensure_secret<TSecrets>(
    secrets: &TSecrets,
    tenant_id: &str,
    cfg: &TelegramConfig,
) -> Result<String>
where
    TSecrets: SecretsManager + Sync + ?Sized,
{
    let existing_secret = secrets.get(&cfg.secret_token_key).await?;
    let secret_missing = existing_secret.is_none();
    let secret = existing_secret.unwrap_or_else(generate_secret);

    if secret_missing && secrets.can_write() {
        if let Err(err) = secrets.put(&cfg.secret_token_key, &secret).await {
            warn!(
                tenant = %tenant_id,
                event = "telegram_webhook_reconcile",
                action = "secret_store_write_failed",
                error = %err
            );
        }
    } else if secret_missing && !secrets.can_write() {
        warn!(
            tenant = %tenant_id,
            event = "telegram_webhook_reconcile",
            action = "secret_missing",
            message = "generated temporary secret; persistence unavailable"
        );
    }

    Ok(secret)
}

pub fn desired_webhook_url(cfg: &TelegramConfig, tenant: &str) -> String {
    format!(
        "{}/{}",
        cfg.public_webhook_base.trim_end_matches('/'),
        tenant
    )
}

pub fn allowed_updates(cfg: &TelegramConfig) -> Vec<String> {
    cfg.allowed_updates()
}

pub fn urls_match(current: &str, desired: &str) -> bool {
    current.trim_end_matches('/') == desired.trim_end_matches('/')
}

pub fn generate_secret() -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut rng = rng();
    (0..32)
        .map(|_| {
            let idx = rng.random_range(0..ALPHABET.len());
            ALPHABET[idx] as char
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;
    use async_trait::async_trait;
    use std::collections::HashMap;
    use tokio::sync::Mutex;

    type WebhookCallLog = Mutex<Vec<(String, String, Vec<String>, bool)>>;

    struct MockApi {
        info: Mutex<WebhookInfo>,
        set_calls: WebhookCallLog,
    }

    impl MockApi {
        fn new(info_url: &str) -> Self {
            Self {
                info: Mutex::new(WebhookInfo {
                    url: info_url.to_string(),
                    extra: Default::default(),
                }),
                set_calls: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl TelegramApi for MockApi {
        async fn get_webhook_info(&self, _bot_token: &str) -> Result<WebhookInfo> {
            Ok(self.info.lock().await.clone())
        }

        async fn set_webhook(
            &self,
            _bot_token: &str,
            url: &str,
            secret: &str,
            allowed_updates: &[String],
            drop_pending: bool,
        ) -> Result<()> {
            self.set_calls.lock().await.push((
                url.to_string(),
                secret.to_string(),
                allowed_updates.to_vec(),
                drop_pending,
            ));
            Ok(())
        }

        async fn delete_webhook(&self, _bot_token: &str, _drop_pending: bool) -> Result<()> {
            Ok(())
        }
    }

    struct MockSecrets {
        data: Mutex<HashMap<String, String>>,
        writable: bool,
    }

    impl MockSecrets {
        fn new(writable: bool) -> Self {
            Self {
                data: Mutex::new(HashMap::new()),
                writable,
            }
        }

        async fn insert(&self, key: &str, value: &str) {
            self.data
                .lock()
                .await
                .insert(key.to_string(), value.to_string());
        }
    }

    #[async_trait]
    impl SecretsManager for MockSecrets {
        async fn get(&self, key: &str) -> Result<Option<String>> {
            Ok(self.data.lock().await.get(key).cloned())
        }

        async fn put(&self, key: &str, value: &str) -> Result<()> {
            if self.writable {
                self.insert(key, value).await;
                Ok(())
            } else {
                Err(anyhow!("read-only"))
            }
        }

        fn can_write(&self) -> bool {
            self.writable
        }
    }

    fn sample_config() -> (Tenant, TelegramConfig) {
        let tenant = Tenant {
            id: "acme".into(),
            telegram: None,
        };
        let cfg = TelegramConfig {
            enabled: true,
            public_webhook_base: "https://webhook".into(),
            secret_token_key: "tenants/acme/telegram/secret_token".into(),
            allowed_updates: Some(vec!["message".into()]),
            drop_pending_on_first_install: Some(true),
        };
        (tenant, cfg)
    }

    #[tokio::test]
    async fn reconciles_when_url_differs() {
        let (tenant, cfg) = sample_config();
        let api = MockApi::new("");
        let secrets = MockSecrets::new(true);
        secrets
            .insert("tenants/acme/telegram/bot_token", "token")
            .await;
        let outcome = reconcile_tenant(&api, &secrets, &tenant, &cfg).await;
        assert_eq!(outcome.result, ReconcileResult::Applied);
        let calls = api.set_calls.lock().await;
        assert_eq!(calls.len(), 1);
        let (url, _secret, allowed, drop_pending) = &calls[0];
        assert_eq!(url, "https://webhook/acme");
        assert_eq!(allowed, &vec!["message".to_string()]);
        assert!(drop_pending);
    }

    #[tokio::test]
    async fn no_update_when_url_matches() {
        let (tenant, cfg) = sample_config();
        let api = MockApi::new("https://webhook/acme");
        let secrets = MockSecrets::new(true);
        secrets
            .insert("tenants/acme/telegram/bot_token", "token")
            .await;
        secrets
            .insert("tenants/acme/telegram/secret_token", "secret")
            .await;
        let outcome = reconcile_tenant(&api, &secrets, &tenant, &cfg).await;
        assert_eq!(outcome.result, ReconcileResult::Noop);
        assert!(api.set_calls.lock().await.is_empty());
        assert_eq!(outcome.secret.unwrap(), "secret");
    }

    #[tokio::test]
    async fn returns_error_outcome_when_bot_token_missing() {
        let (tenant, cfg) = sample_config();
        let api = MockApi::new("");
        let secrets = MockSecrets::new(false);
        let outcome = reconcile_tenant(&api, &secrets, &tenant, &cfg).await;
        assert_eq!(outcome.result, ReconcileResult::Error);
        assert!(outcome.secret.is_none());
    }

    #[tokio::test]
    async fn generates_and_persists_secret_when_missing() {
        let (tenant, cfg) = sample_config();
        let api = MockApi::new("");
        let secrets = MockSecrets::new(true);
        secrets
            .insert("tenants/acme/telegram/bot_token", "token")
            .await;
        let outcome = reconcile_tenant(&api, &secrets, &tenant, &cfg).await;
        assert_eq!(outcome.result, ReconcileResult::Applied);
        let secret = outcome.secret.expect("secret generated");
        assert_eq!(secret.len(), 32);
        let stored = secrets
            .data
            .lock()
            .await
            .get("tenants/acme/telegram/secret_token")
            .cloned()
            .unwrap();
        assert_eq!(secret, stored);
    }
}
