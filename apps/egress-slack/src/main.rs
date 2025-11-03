//! Slack egress adapter that converts normalized messages into Slack API calls.

use anyhow::Result;
use async_nats::jetstream::AckKind;
use async_trait::async_trait;
use futures::StreamExt;
use gsm_backpressure::BackpressureLimiter;
use gsm_core::egress::{EgressSender, OutboundMessage, SendResult};
use gsm_core::platforms::slack::sender::SlackSender;
use gsm_core::prelude::DefaultResolver;
use gsm_core::{NodeError, OutMessage, Platform, TenantCtx};
use gsm_dlq::{DlqError, DlqPublisher};
use gsm_egress_common::{
    egress::bootstrap,
    telemetry::{context_from_out, record_egress_success, start_acquire_span, start_send_span},
};
use gsm_telemetry::install as init_telemetry;
use gsm_translator::slack::to_slack_payloads;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use tokio::time::sleep;
use tracing::Instrument;

type NodeErrorResult<T> = Result<T, NodeError>;

const MAX_ATTEMPTS: usize = 3;

#[tokio::main]
async fn main() -> Result<()> {
    init_telemetry("greentic-messaging")?;
    let nats_url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
    let tenant = std::env::var("TENANT").unwrap_or_else(|_| "acme".into());

    let queue = bootstrap(&nats_url, &tenant, Platform::Slack.as_str()).await?;
    tracing::info!(
        stream = %queue.stream,
        consumer = %queue.consumer,
        "egress-slack consuming from JetStream"
    );

    let dlq = Arc::new(DlqPublisher::new("egress", queue.client()).await?);
    let limiter = queue.limiter.clone();
    let resolver = Arc::new(DefaultResolver::new().await?);
    #[cfg(feature = "mock-http")]
    // In CI we avoid real Slack calls by forcing the sender to use a mock base URL.
    let api_base = Some("mock://slack".into());
    #[cfg(not(feature = "mock-http"))]
    let api_base = std::env::var("SLACK_API_BASE").ok();
    let sender = Arc::new(SlackSender::new(reqwest::Client::new(), resolver, api_base));

    let mut messages = queue.messages;

    while let Some(next) = messages.next().await {
        let msg = match next {
            Ok(msg) => msg,
            Err(err) => {
                tracing::error!(error = %err, "jetstream message error");
                continue;
            }
        };

        if let Err(err) =
            handle_message(limiter.as_ref(), sender.as_ref(), dlq.as_ref(), &msg).await
        {
            tracing::error!(error = %err, "failed to process slack message");
            if let Err(nak) = msg.ack_with(AckKind::Nak(None)).await {
                tracing::error!(error = %nak, "failed to nack message");
            }
        }
    }

    Ok(())
}

async fn handle_message<S, D, M>(
    limiter: &dyn BackpressureLimiter,
    sender: &S,
    dlq: &D,
    msg: &M,
) -> Result<()>
where
    S: EgressSender + Send + Sync,
    D: DlqSink + Send + Sync,
    M: DeliveryMessage + Send + Sync,
{
    let out: OutMessage = match serde_json::from_slice(msg.payload()) {
        Ok(v) => v,
        Err(err) => {
            tracing::warn!(error = %err, "failed to decode OutMessage payload");
            msg.ack().await?;
            return Ok(());
        }
    };

    if out.platform != Platform::Slack {
        msg.ack().await?;
        return Ok(());
    }

    let ctx = context_from_out(&out);
    let msg_id = ctx
        .labels
        .msg_id
        .clone()
        .unwrap_or_else(|| out.message_id());

    let acquire_span = start_acquire_span(&ctx);
    let _permit = limiter
        .acquire(&out.tenant)
        .instrument(acquire_span)
        .await?;

    let send_span = start_send_span(&ctx);
    let start_time = Instant::now();

    let payloads = match to_slack_payloads(&out) {
        Ok(payloads) => payloads,
        Err(err) => {
            tracing::warn!(error = %err, "slack translation failed");
            msg.ack_with(AckKind::Nak(None)).await?;
            return Ok(());
        }
    };

    let mut error: Option<NodeError> = None;
    {
        let _guard = send_span.enter();
        for payload in payloads {
            let outbound = OutboundMessage {
                channel: Some(out.chat_id.clone()),
                text: out.text.clone(),
                payload: Some(payload.clone()),
            };
            match send_with_retries(sender, &out.ctx, &outbound).await {
                Ok(_res) => {
                    tracing::debug!(
                        env = %out.ctx.env.as_str(),
                        tenant = %out.tenant,
                        chat_id = %out.chat_id,
                        "slack message posted"
                    );
                }
                Err(err) => {
                    tracing::warn!(
                        env = %out.ctx.env.as_str(),
                        tenant = %out.tenant,
                        chat_id = %out.chat_id,
                        error = %err,
                        "slack send failed"
                    );
                    error = Some(err);
                    break;
                }
            }
        }
    }

    if let Some(err) = error {
        if err.retryable {
            tracing::warn!(
                backoff_ms = err.backoff_ms,
                "retryable slack error; nacking"
            );
            msg.ack_with(AckKind::Nak(None)).await?;
        } else {
            let code = err.code.clone();
            let message = err.message.clone();
            let dlq_err = DlqError {
                code: code.clone(),
                message: message.clone(),
                stage: None,
            };
            dlq.publish_dlq(&out.tenant, out.platform.as_str(), &msg_id, dlq_err, &out)
                .await?;
            msg.ack().await?;
        }
    } else {
        msg.ack().await?;
        let elapsed_ms = start_time.elapsed().as_secs_f64() * 1000.0;
        record_egress_success(&ctx, elapsed_ms);
    }

    Ok(())
}

async fn send_with_retries<S>(
    sender: &S,
    ctx: &TenantCtx,
    msg: &OutboundMessage,
) -> NodeErrorResult<SendResult>
where
    S: EgressSender + Send + Sync,
{
    let mut attempt = 0;
    loop {
        attempt += 1;
        match sender.send(ctx, msg.clone()).await {
            Ok(res) => return Ok(res),
            Err(err) => {
                let retryable = err.retryable;
                let backoff_ms = err.backoff_ms;
                if retryable && attempt < MAX_ATTEMPTS {
                    let delay = backoff_ms
                        .map(Duration::from_millis)
                        .unwrap_or_else(|| Duration::from_secs(attempt as u64));
                    tracing::warn!(attempt, delay_ms = delay.as_millis(), "slack retry");
                    sleep(delay).await;
                    continue;
                } else {
                    return Err(err);
                }
            }
        }
    }
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
    async fn ack_with(&self, kind: AckKind) -> Result<()>;
}

#[async_trait::async_trait]
impl DeliveryMessage for async_nats::jetstream::Message {
    fn payload(&self) -> &[u8] {
        self.payload.as_ref()
    }

    async fn ack(&self) -> Result<()> {
        self.ack()
            .await
            .map_err(|err| anyhow::Error::msg(err.to_string()))?;
        Ok(())
    }

    async fn ack_with(&self, kind: AckKind) -> Result<()> {
        self.ack_with(kind)
            .await
            .map_err(|err| anyhow::Error::msg(err.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gsm_backpressure::{LocalBackpressureLimiter, RateLimits};
    use gsm_core::{OutKind, OutMessage, Platform, make_tenant_ctx};
    use serde_json::json;
    use std::{
        collections::BTreeMap,
        sync::{Arc, Mutex},
    };
    use tokio::sync::Mutex as AsyncMutex;

    struct MockSender {
        responses: AsyncMutex<Vec<NodeErrorResult<SendResult>>>,
        calls: AsyncMutex<usize>,
    }

    impl MockSender {
        fn new(responses: Vec<NodeErrorResult<SendResult>>) -> Self {
            Self {
                responses: AsyncMutex::new(responses),
                calls: AsyncMutex::new(0),
            }
        }

        async fn call_count(&self) -> usize {
            *self.calls.lock().await
        }
    }

    #[async_trait::async_trait]
    impl EgressSender for MockSender {
        async fn send(
            &self,
            _ctx: &TenantCtx,
            _msg: OutboundMessage,
        ) -> NodeErrorResult<SendResult> {
            let mut calls = self.calls.lock().await;
            *calls += 1;
            let mut responses = self.responses.lock().await;
            if responses.is_empty() {
                Ok(SendResult::default())
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

    #[async_trait::async_trait]
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
        acked: AsyncMutex<bool>,
    }

    impl MockMessage {
        fn new(out: &OutMessage) -> Self {
            Self {
                payload: serde_json::to_vec(out).unwrap(),
                acked: AsyncMutex::new(false),
            }
        }

        async fn acked(&self) -> bool {
            *self.acked.lock().await
        }
    }

    #[async_trait::async_trait]
    impl DeliveryMessage for MockMessage {
        fn payload(&self) -> &[u8] {
            &self.payload
        }

        async fn ack(&self) -> Result<()> {
            *self.acked.lock().await = true;
            Ok(())
        }

        async fn ack_with(&self, _kind: AckKind) -> Result<()> {
            *self.acked.lock().await = true;
            Ok(())
        }
    }

    fn sample_out() -> OutMessage {
        let mut meta = BTreeMap::new();
        meta.insert("source_msg_id".into(), json!("mid-1"));
        OutMessage {
            ctx: make_tenant_ctx("acme".into(), Some("team".into()), None),
            tenant: "acme".into(),
            platform: Platform::Slack,
            chat_id: "C123".into(),
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
    async fn retries_on_retryable_error_then_succeeds() {
        let retry_err = NodeError::new("slack_send_failed", "rate").with_retry(Some(1));
        let sender = MockSender::new(vec![Err(retry_err), Ok(SendResult::default())]);
        let out = sample_out();
        let outbound = OutboundMessage {
            channel: Some(out.chat_id.clone()),
            text: out.text.clone(),
            payload: Some(json!({"text": out.text.clone().unwrap()})),
        };
        send_with_retries(&sender, &out.ctx, &outbound)
            .await
            .unwrap();
        assert_eq!(sender.call_count().await, 2);
    }

    #[tokio::test]
    async fn handle_message_success_ack() {
        let sender = MockSender::new(vec![Ok(SendResult::default())]);
        let dlq = MockDlq::new();
        let lim = limiter();
        let out = sample_out();
        let message = MockMessage::new(&out);

        handle_message(lim.as_ref(), &sender, &dlq, &message)
            .await
            .unwrap();

        assert!(message.acked().await);
        assert!(dlq.entries.lock().unwrap().is_empty());
        assert_eq!(sender.call_count().await, 1);
    }

    #[tokio::test]
    async fn handle_message_dlq_on_non_retryable_failure() {
        let sender = MockSender::new(vec![Err(NodeError::new("slack_send_failed", "bad"))]);
        let dlq = MockDlq::new();
        let lim = limiter();
        let out = sample_out();
        let message = MockMessage::new(&out);

        handle_message(lim.as_ref(), &sender, &dlq, &message)
            .await
            .unwrap();

        assert!(message.acked().await);
        {
            let entries = dlq.entries.lock().unwrap();
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].code, "slack_send_failed");
        }
        assert_eq!(sender.call_count().await, 1);
    }
}
