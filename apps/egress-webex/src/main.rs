mod client;

use anyhow::{anyhow, Result};
use async_nats::jetstream::{self, AckKind};
use async_trait::async_trait;
use futures::StreamExt;
use gsm_backpressure::BackpressureLimiter;
use gsm_core::{OutMessage, Platform};
use gsm_dlq::{DlqError, DlqPublisher};
use gsm_egress_common::{
    bootstrap, context_from_out, record_egress_success, start_acquire_span, start_send_span,
};
use gsm_telemetry::{init_telemetry, TelemetryConfig};
use std::{sync::Arc, time::Duration};
use tokio::time::sleep;
use tracing::{error, info, warn, Instrument};

use crate::client::{WebexClient, WebexError, WebexSender};

const MAX_ATTEMPTS: usize = 3;

#[tokio::main]
async fn main() -> Result<()> {
    let telemetry = TelemetryConfig::from_env("gsm-egress-webex", env!("CARGO_PKG_VERSION"));
    init_telemetry(telemetry)?;

    let nats_url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
    let tenant = std::env::var("TENANT").unwrap_or_else(|_| "acme".into());
    let token = std::env::var("WEBEX_BOT_TOKEN").expect("WEBEX_BOT_TOKEN env var required");
    let api_base = std::env::var("WEBEX_API_BASE").ok();

    let queue = bootstrap(&nats_url, &tenant, Platform::Webex.as_str()).await?;
    info!(stream = %queue.stream, consumer = %queue.consumer, "egress-webex consuming from JetStream");

    let dlq = Arc::new(DlqPublisher::new("egress", queue.client()).await?);
    let limiter = queue.limiter.clone();
    let client = Arc::new(WebexClient::new(token, api_base)?);

    let mut messages = queue.messages;

    while let Some(next) = messages.next().await {
        let msg = match next {
            Ok(msg) => msg,
            Err(err) => {
                error!(error = %err, "jetstream message error");
                continue;
            }
        };

        if let Err(err) = handle_message(
            &tenant,
            limiter.as_ref(),
            client.as_ref(),
            dlq.as_ref(),
            &msg,
        )
        .await
        {
            error!(error = %err, "failed to process webex message");
            if let Err(ack_err) = msg.ack_with(AckKind::Nak(None)).await {
                error!(error = %ack_err, "failed to nack message");
            }
        }
    }

    Ok(())
}

#[async_trait]
trait DlqSink: Send + Sync {
    async fn publish_dlq(
        &self,
        tenant: &str,
        platform: &str,
        msg_id: &str,
        error: DlqError,
        payload: &OutMessage,
    ) -> Result<()>;
}

#[async_trait]
impl DlqSink for DlqPublisher {
    async fn publish_dlq(
        &self,
        tenant: &str,
        platform: &str,
        msg_id: &str,
        error: DlqError,
        payload: &OutMessage,
    ) -> Result<()> {
        self.publish(tenant, platform, msg_id, 1, error, payload)
            .await
    }
}

#[async_trait]
trait DeliveryMessage {
    fn payload(&self) -> &[u8];
    async fn ack(&self) -> Result<()>;
}

#[async_trait]
impl DeliveryMessage for jetstream::Message {
    fn payload(&self) -> &[u8] {
        self.payload.as_ref()
    }

    async fn ack(&self) -> Result<()> {
        self.ack().await.map_err(|err| anyhow!(err))?;
        Ok(())
    }
}

async fn handle_message(
    tenant: &str,
    limiter: &dyn BackpressureLimiter,
    sender: &(dyn WebexSender + Send + Sync),
    dlq: &(dyn DlqSink + Send + Sync),
    msg: &(dyn DeliveryMessage + Send + Sync),
) -> Result<()> {
    let out: OutMessage = match serde_json::from_slice(msg.payload()) {
        Ok(out) => out,
        Err(err) => {
            error!(error = %err, "failed to decode OutMessage payload");
            msg.ack().await?;
            return Ok(());
        }
    };

    if out.platform != Platform::Webex {
        msg.ack().await?;
        return Ok(());
    }

    let ctx = context_from_out(&out);
    let acquire_span = start_acquire_span(&ctx);
    let permit = limiter
        .acquire(&out.tenant)
        .instrument(acquire_span)
        .await?;

    let send_span = start_send_span(&ctx);
    let start_time = std::time::Instant::now();

    let result = {
        let _guard = send_span.enter();
        send_with_retries(sender, &out).await
    };
    drop(permit);

    match result {
        Ok(()) => {
            msg.ack().await?;
            let elapsed_ms = start_time.elapsed().as_secs_f64() * 1000.0;
            record_egress_success(&ctx, elapsed_ms);
            info!(chat_id = %out.chat_id, "webex message sent");
        }
        Err(err) => {
            error!(error = %err, "webex send failed");
            let dlq_err = DlqError {
                code: match &err {
                    SendError::Webex(WebexError::RateLimited { .. }) => "E_RATE".into(),
                    SendError::Webex(WebexError::Server { .. }) => "E_SERVER".into(),
                    SendError::Webex(WebexError::Client { .. }) => "E_CLIENT".into(),
                    SendError::Webex(WebexError::Serialization(_)) => "E_SERIAL".into(),
                    SendError::Webex(WebexError::Transport(_)) => "E_TRANSPORT".into(),
                    SendError::Other(_) => "E_UNKNOWN".into(),
                },
                message: err.to_string(),
                stage: None,
            };
            dlq.publish_dlq(
                tenant,
                out.platform.as_str(),
                &out.message_id(),
                dlq_err,
                &out,
            )
            .await?;
            msg.ack().await?;
        }
    }

    Ok(())
}

#[derive(thiserror::Error, Debug)]
enum SendError {
    #[error("webex error: {0}")]
    Webex(#[from] WebexError),
    #[error("other error: {0}")]
    Other(#[from] anyhow::Error),
}

async fn send_with_retries(
    sender: &(dyn WebexSender + Send + Sync),
    out: &OutMessage,
) -> Result<(), SendError> {
    let mut attempt = 0;
    loop {
        attempt += 1;
        match sender.send(out).await {
            Ok(()) => return Ok(()),
            Err(WebexError::RateLimited { retry_after, .. }) if attempt < MAX_ATTEMPTS => {
                let delay = retry_after.unwrap_or_else(|| Duration::from_secs(1));
                warn!(attempt, ?delay, "webex rate limited; retrying");
                sleep(delay).await;
            }
            Err(WebexError::Server { .. }) if attempt < MAX_ATTEMPTS => {
                let delay = Duration::from_secs(attempt as u64);
                warn!(attempt, "webex server error; retrying");
                sleep(delay).await;
            }
            Err(err) => return Err(SendError::Webex(err)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::WebexError;
    use gsm_backpressure::{LocalBackpressureLimiter, RateLimits};
    use gsm_core::OutKind;
    use reqwest::StatusCode;
    use serde_json::Value;
    use std::{
        collections::BTreeMap,
        sync::{Arc, Mutex},
    };
    use tokio::sync::Mutex as AsyncMutex;

    struct MockSender {
        responses: AsyncMutex<Vec<Result<(), WebexError>>>,
        calls: AsyncMutex<usize>,
    }

    impl MockSender {
        fn new(responses: Vec<Result<(), WebexError>>) -> Self {
            Self {
                responses: AsyncMutex::new(responses),
                calls: AsyncMutex::new(0),
            }
        }

        async fn call_count(&self) -> usize {
            *self.calls.lock().await
        }
    }

    #[async_trait]
    impl WebexSender for MockSender {
        async fn send(&self, _out: &OutMessage) -> Result<(), WebexError> {
            let mut calls = self.calls.lock().await;
            *calls += 1;
            let mut responses = self.responses.lock().await;
            if responses.is_empty() {
                Ok(())
            } else {
                responses.remove(0)
            }
        }
    }

    struct MockDlq {
        entries: Mutex<Vec<DlqError>>,
    }

    impl MockDlq {
        fn new() -> Self {
            Self {
                entries: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl DlqSink for MockDlq {
        async fn publish_dlq(
            &self,
            _tenant: &str,
            _platform: &str,
            _msg_id: &str,
            error: DlqError,
            _payload: &OutMessage,
        ) -> Result<()> {
            self.entries.lock().unwrap().push(error);
            Ok(())
        }
    }

    struct MockMessage {
        payload: Vec<u8>,
        acked: Mutex<bool>,
    }

    impl MockMessage {
        fn new(out: &OutMessage) -> Self {
            Self {
                payload: serde_json::to_vec(out).unwrap(),
                acked: Mutex::new(false),
            }
        }

        fn acked(&self) -> bool {
            *self.acked.lock().unwrap()
        }
    }

    #[async_trait]
    impl DeliveryMessage for MockMessage {
        fn payload(&self) -> &[u8] {
            &self.payload
        }

        async fn ack(&self) -> Result<()> {
            *self.acked.lock().unwrap() = true;
            Ok(())
        }
    }

    fn sample_out() -> OutMessage {
        let mut meta = BTreeMap::new();
        meta.insert("source_msg_id".into(), Value::String("mid-1".into()));
        OutMessage {
            tenant: "acme".into(),
            platform: Platform::Webex,
            chat_id: "room-42".into(),
            thread_id: None,
            kind: OutKind::Text,
            text: Some("hi".into()),
            message_card: None,
            meta,
        }
    }

    fn limiter() -> Arc<LocalBackpressureLimiter> {
        let limits = Arc::new(RateLimits::from_env());
        Arc::new(LocalBackpressureLimiter::new(limits))
    }

    #[tokio::test]
    async fn retries_on_rate_limit_then_succeeds() {
        let retry_err = WebexError::RateLimited {
            retry_after: Some(Duration::from_millis(1)),
            body: "limit".into(),
        };
        let sender = MockSender::new(vec![Err(retry_err), Ok(())]);
        let out = sample_out();
        send_with_retries(&sender, &out).await.unwrap();
        assert_eq!(sender.call_count().await, 2);
    }

    #[tokio::test]
    async fn handle_message_success_ack() {
        let sender = MockSender::new(vec![Ok(())]);
        let dlq = MockDlq::new();
        let lim = limiter();
        let out = sample_out();
        let message = MockMessage::new(&out);

        handle_message("acme", lim.as_ref(), &sender, &dlq, &message)
            .await
            .unwrap();

        assert!(message.acked());
        assert!(dlq.entries.lock().unwrap().is_empty());
        assert_eq!(sender.call_count().await, 1);
    }

    #[tokio::test]
    async fn handle_message_dlq_on_failure() {
        let sender = MockSender::new(vec![Err(WebexError::Client {
            status: StatusCode::BAD_REQUEST,
            body: "bad".into(),
        })]);
        let dlq = MockDlq::new();
        let lim = limiter();
        let out = sample_out();
        let message = MockMessage::new(&out);

        handle_message("acme", lim.as_ref(), &sender, &dlq, &message)
            .await
            .unwrap();

        assert!(message.acked());
        let entries = dlq.entries.lock().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].code, "E_CLIENT");
        drop(entries);
        assert_eq!(sender.call_count().await, 1);
    }
}
