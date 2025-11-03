use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashMap, time::Duration};
use tokio::time::sleep;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct WebhookInfo {
    #[serde(default)]
    pub url: String,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize)]
struct TelegramResponse<T> {
    ok: bool,
    result: Option<T>,
    description: Option<String>,
}

#[async_trait]
pub trait TelegramApi: Send + Sync {
    async fn get_webhook_info(&self, bot_token: &str) -> Result<WebhookInfo>;
    async fn set_webhook(
        &self,
        bot_token: &str,
        url: &str,
        secret: &str,
        allowed_updates: &[String],
        drop_pending: bool,
    ) -> Result<()>;
    #[allow(dead_code)]
    async fn delete_webhook(&self, bot_token: &str, drop_pending: bool) -> Result<()>;
}

#[derive(Clone)]
pub struct HttpTelegramApi {
    client: Client,
    api_base: String,
}

impl HttpTelegramApi {
    pub fn new(client: Client, api_base: Option<String>) -> Self {
        let api_base = api_base.unwrap_or_else(|| "https://api.telegram.org".into());
        Self { client, api_base }
    }

    fn url(&self, bot_token: &str, method: &str) -> String {
        format!(
            "{}/bot{}/{}",
            self.api_base.trim_end_matches('/'),
            bot_token,
            method
        )
    }

    async fn with_retry<F, Fut, T>(mut op: F) -> Result<T>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = Result<T>>,
    {
        let mut attempt = 0;
        let delays = [
            Duration::from_millis(250),
            Duration::from_secs(1),
            Duration::from_secs(4),
        ];
        loop {
            match op().await {
                Ok(value) => return Ok(value),
                Err(_err) if attempt < delays.len() => {
                    sleep(delays[attempt]).await;
                    attempt += 1;
                    continue;
                }
                Err(err) => return Err(err),
            }
        }
    }
}

#[async_trait]
impl TelegramApi for HttpTelegramApi {
    async fn get_webhook_info(&self, bot_token: &str) -> Result<WebhookInfo> {
        let url = self.url(bot_token, "getWebhookInfo");
        Self::with_retry(|| async {
            let res = self
                .client
                .get(&url)
                .timeout(Duration::from_secs(5))
                .send()
                .await
                .context("telegram getWebhookInfo request")?;
            let status = res.status();
            if !status.is_success() {
                let body = res.text().await.unwrap_or_default();
                return Err(anyhow!("telegram getWebhookInfo {}: {}", status, body));
            }
            let body: TelegramResponse<WebhookInfo> = res
                .json()
                .await
                .context("decode telegram getWebhookInfo response")?;
            if body.ok {
                Ok(body.result.unwrap_or_default())
            } else {
                Err(anyhow!(
                    "telegram getWebhookInfo failed: {}",
                    body.description.unwrap_or_else(|| "unknown error".into())
                ))
            }
        })
        .await
    }

    async fn set_webhook(
        &self,
        bot_token: &str,
        url: &str,
        secret: &str,
        allowed_updates: &[String],
        drop_pending: bool,
    ) -> Result<()> {
        let endpoint = self.url(bot_token, "setWebhook");
        let payload = serde_json::json!({
            "url": url,
            "secret_token": secret,
            "allowed_updates": allowed_updates,
            "drop_pending_updates": drop_pending,
        });
        Self::with_retry(|| async {
            let res = self
                .client
                .post(&endpoint)
                .timeout(Duration::from_secs(5))
                .json(&payload)
                .send()
                .await
                .context("telegram setWebhook request")?;
            let status = res.status();
            if !status.is_success() {
                let body = res.text().await.unwrap_or_default();
                return Err(anyhow!("telegram setWebhook {}: {}", status, body));
            }
            let body: TelegramResponse<serde_json::Value> = res
                .json()
                .await
                .context("decode telegram setWebhook response")?;
            if body.ok {
                Ok(())
            } else {
                Err(anyhow!(
                    "telegram setWebhook failed: {}",
                    body.description.unwrap_or_else(|| "unknown error".into())
                ))
            }
        })
        .await
    }

    #[allow(dead_code)]
    async fn delete_webhook(&self, bot_token: &str, drop_pending: bool) -> Result<()> {
        let endpoint = self.url(bot_token, "deleteWebhook");
        let payload = serde_json::json!({
            "drop_pending_updates": drop_pending,
        });
        Self::with_retry(|| async {
            let res = self
                .client
                .post(&endpoint)
                .timeout(Duration::from_secs(5))
                .json(&payload)
                .send()
                .await
                .context("telegram deleteWebhook request")?;
            let status = res.status();
            if !status.is_success() {
                let body = res.text().await.unwrap_or_default();
                return Err(anyhow!("telegram deleteWebhook {}: {}", status, body));
            }
            let body: TelegramResponse<serde_json::Value> = res
                .json()
                .await
                .context("decode telegram deleteWebhook response")?;
            if body.ok {
                Ok(())
            } else {
                Err(anyhow!(
                    "telegram deleteWebhook failed: {}",
                    body.description.unwrap_or_else(|| "unknown error".into())
                ))
            }
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    #[tokio::test]
    async fn retry_returns_success_on_second_attempt() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let result: Result<u8> = HttpTelegramApi::with_retry({
            let attempts = attempts.clone();
            move || {
                let attempts = attempts.clone();
                async move {
                    let current = attempts.fetch_add(1, Ordering::SeqCst);
                    if current < 1 {
                        Err(anyhow!("boom"))
                    } else {
                        Ok(5)
                    }
                }
            }
        })
        .await;
        assert_eq!(result.unwrap(), 5);
        assert!(attempts.load(Ordering::SeqCst) >= 2);
    }

    #[tokio::test]
    async fn retry_exhausts_attempts() {
        let result: Result<()> =
            HttpTelegramApi::with_retry(|| async { Err(anyhow!("nope")) }).await;
        assert!(result.is_err());
    }

    #[test]
    fn telegram_response_deserializes() {
        let body = json!({
            "ok": true,
            "result": {
                "url": "https://example"
            }
        });
        let parsed: TelegramResponse<WebhookInfo> = serde_json::from_value(body).unwrap();
        assert!(parsed.ok);
        assert_eq!(parsed.result.unwrap().url, "https://example");
    }
}
