use anyhow::Result;
use async_trait::async_trait;
use reqwest::{header, StatusCode};
use std::time::Duration;
use thiserror::Error;

use gsm_core::{OutMessage, TenantCtx};
use gsm_translator::webex::to_webex_payload;

#[derive(Clone)]
pub struct WebexClient {
    http: reqwest::Client,
    base_url: String,
    token: String,
}

impl WebexClient {
    pub fn new(token: String, base_url: Option<String>) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent("greentic-egress-webex/0.1.0")
            .build()?;
        Ok(Self {
            http,
            base_url: base_url.unwrap_or_else(|| "https://webexapis.com/v1".into()),
            token,
        })
    }

    pub async fn send_message(&self, out: &OutMessage) -> Result<(), WebexError> {
        let payload = to_webex_payload(out)
            .map_err(|err: anyhow::Error| WebexError::Serialization(err.to_string()))?;
        let url = format!("{}/messages", self.base_url.trim_end_matches('/'));
        let res = self
            .http
            .post(url)
            .header(header::AUTHORIZATION, format!("Bearer {}", self.token))
            .json(&payload)
            .send()
            .await
            .map_err(|err| WebexError::Transport(err.into()))?;

        classify_response(res).await
    }
}

async fn classify_response(res: reqwest::Response) -> Result<(), WebexError> {
    let status = res.status();
    let retry_header = res.headers().get(header::RETRY_AFTER).cloned();
    if status.is_success() {
        return Ok(());
    }

    let body = res.text().await.unwrap_or_else(|_| "<empty>".to_string());

    if status == StatusCode::TOO_MANY_REQUESTS {
        let retry_after = retry_header
            .as_ref()
            .and_then(|value| value.to_str().ok())
            .and_then(parse_retry_after);
        return Err(WebexError::RateLimited { retry_after, body });
    }

    if status.is_server_error() {
        return Err(WebexError::Server { status, body });
    }

    Err(WebexError::Client { status, body })
}

fn parse_retry_after(value: &str) -> Option<Duration> {
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc2822)
        .ok()
        .and_then(|time| {
            let now = time::OffsetDateTime::now_utc();
            let delta = time - now;
            if delta.is_positive() {
                Some(delta.unsigned_abs())
            } else {
                None
            }
        })
}

#[derive(Debug, Error)]
pub enum WebexError {
    #[error("rate limited: {body}")]
    RateLimited {
        retry_after: Option<Duration>,
        body: String,
    },
    #[error("server error {status}: {body}")]
    Server { status: StatusCode, body: String },
    #[error("client error {status}: {body}")]
    Client { status: StatusCode, body: String },
    #[error("payload serialization failed: {0}")]
    Serialization(String),
    #[error("transport error: {0}")]
    Transport(anyhow::Error),
}

#[async_trait]
pub trait WebexSender: Send + Sync {
    async fn send(&self, ctx: &TenantCtx, out: &OutMessage) -> Result<(), WebexError>;
}

#[async_trait]
impl WebexSender for WebexClient {
    async fn send(&self, ctx: &TenantCtx, out: &OutMessage) -> Result<(), WebexError> {
        tracing::debug!(
            env = %ctx.env.as_str(),
            tenant = %ctx.tenant.as_str(),
            chat_id = %out.chat_id,
            msg_id = %out.message_id(),
            "sending webex message"
        );
        self.send_message(out).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use time::{Duration as TimeDuration, OffsetDateTime};

    #[test]
    fn parses_retry_after_seconds() {
        let parsed = parse_retry_after("10").expect("retry");
        assert_eq!(parsed, Duration::from_secs(10));
    }

    #[test]
    fn parses_retry_after_rfc2822() {
        let future = OffsetDateTime::now_utc() + TimeDuration::seconds(5);
        let header = future
            .format(&time::format_description::well_known::Rfc2822)
            .unwrap();
        let parsed = parse_retry_after(&header).expect("retry");
        assert!(parsed >= Duration::from_secs(4));
    }
}
