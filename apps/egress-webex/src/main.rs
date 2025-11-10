use anyhow::{Result, anyhow};
use async_nats::jetstream::{self, AckKind};
use async_trait::async_trait;
use futures::StreamExt;
use gsm_backpressure::BackpressureLimiter;
use gsm_core::NodeResult;
use gsm_core::egress::{EgressSender, OutboundMessage, SendResult};
use gsm_core::messaging_card::{MessageCardKind, ensure_oauth_start_url};
use gsm_core::oauth::{OauthClient, ReqwestTransport};
use gsm_core::platforms::webex::sender::WebexSender;
use gsm_core::prelude::DefaultResolver;
use gsm_core::{OutMessage, Platform, TenantCtx};
use gsm_dlq::{DlqError, DlqPublisher};
use gsm_egress_common::{
    bootstrap,
    telemetry::{
        AuthRenderMode, context_from_out, record_auth_card_render, record_egress_success,
        start_acquire_span, start_send_span,
    },
};
use gsm_telemetry::install as init_telemetry;
use gsm_translator::webex::to_webex_payload;
use std::{sync::Arc, time::Duration};
use tokio::time::sleep;
use tracing::{Instrument, error, info, warn};

const MAX_ATTEMPTS: usize = 3;

#[tokio::main]
async fn main() -> Result<()> {
    init_telemetry("greentic-messaging")?;
    let nats_url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
    let tenant = std::env::var("TENANT").unwrap_or_else(|_| "acme".into());
    #[cfg(feature = "mock-http")]
    // Short-circuit outbound Webex traffic during tests.
    let api_base = Some("mock://webex".into());
    #[cfg(not(feature = "mock-http"))]
    let api_base = std::env::var("WEBEX_API_BASE").ok();

    let queue = bootstrap(&nats_url, &tenant, Platform::Webex.as_str()).await?;
    info!(stream = %queue.stream, consumer = %queue.consumer, "egress-webex consuming from JetStream");

    let dlq = Arc::new(DlqPublisher::new("egress", queue.client()).await?);
    let limiter = queue.limiter.clone();
    let resolver = Arc::new(DefaultResolver::new().await?);
    let sender = Arc::new(WebexSender::new(
        reqwest::Client::new(),
        resolver,
        api_base.clone(),
    ));

    let oauth_client = match std::env::var("OAUTH_BASE_URL") {
        Ok(_) => match OauthClient::from_env(reqwest::Client::new()) {
            Ok(client) => {
                tracing::info!("OAUTH_BASE_URL detected; Webex OAuth builder enabled");
                Some(Arc::new(client))
            }
            Err(err) => {
                warn!(error = %err, "failed to initialize Webex OAuth client");
                None
            }
        },
        Err(_) => None,
    };

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
            sender.as_ref(),
            dlq.as_ref(),
            oauth_client.as_deref(),
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
    async fn ack_with(&self, kind: AckKind) -> Result<()>;
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

    async fn ack_with(&self, kind: AckKind) -> Result<()> {
        self.ack_with(kind).await.map_err(|err| anyhow!(err))?;
        Ok(())
    }
}

async fn handle_message(
    tenant: &str,
    limiter: &dyn BackpressureLimiter,
    sender: &(dyn EgressSender + Send + Sync),
    dlq: &(dyn DlqSink + Send + Sync),
    oauth_client: Option<&OauthClient<ReqwestTransport>>,
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
    let _permit = limiter
        .acquire(&out.tenant)
        .instrument(acquire_span)
        .await?;

    let send_span = start_send_span(&ctx);
    let start_time = std::time::Instant::now();

    let mut translated_out = out.clone();
    let mut drop_adaptive = false;
    if let (Some(card), Some(client)) = (translated_out.adaptive_card.as_mut(), oauth_client)
        && matches!(card.kind, MessageCardKind::Oauth)
        && let Err(err) = ensure_oauth_start_url(card, &translated_out.ctx, client, None).await
    {
        warn!(error = %err, "failed to build oauth start_url; downgrading for webex");
        drop_adaptive = true;
    }
    if drop_adaptive {
        translated_out.adaptive_card = None;
    }

    if let Some(card) = translated_out.adaptive_card.as_ref()
        && matches!(card.kind, MessageCardKind::Oauth)
        && let Some(oauth) = card.oauth.as_ref()
    {
        let team = out.ctx.team.as_ref().map(|team| team.as_ref());
        record_auth_card_render(
            &ctx,
            oauth.provider.as_str(),
            AuthRenderMode::Downgrade,
            oauth.connection_name.as_deref(),
            oauth.start_url.as_deref(),
            team,
        );
    }

    let payload = match to_webex_payload(&translated_out) {
        Ok(payload) => payload,
        Err(err) => {
            warn!(error = %err, "webex payload translation failed");
            msg.ack().await?;
            return Ok(());
        }
    };

    let outbound = OutboundMessage {
        channel: Some(out.chat_id.clone()),
        text: out.text.clone(),
        payload: Some(payload),
    };

    let result = {
        let _guard = send_span.enter();
        send_with_retries(sender, &out.ctx, &outbound).await
    };
    match result {
        Ok(send_result) => {
            msg.ack().await?;
            let elapsed_ms = start_time.elapsed().as_secs_f64() * 1000.0;
            record_egress_success(&ctx, elapsed_ms);
            info!(
                chat_id = %out.chat_id,
                message_id = ?send_result.message_id,
                "webex message sent"
            );
        }
        Err(err) => {
            if err.retryable {
                warn!(
                    tenant = tenant,
                    chat_id = %out.chat_id,
                    "webex retryable failure, nacking"
                );
                msg.ack_with(AckKind::Nak(None)).await?;
            } else {
                let code = err.code.clone();
                let message = err.message.clone();
                error!(
                    tenant = tenant,
                    chat_id = %out.chat_id,
                    code = %code,
                    %message,
                    "webex send failed"
                );
                let dlq_err = DlqError {
                    code: code.clone(),
                    message: message.clone(),
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
    }

    Ok(())
}

async fn send_with_retries(
    sender: &(dyn EgressSender + Send + Sync),
    ctx: &TenantCtx,
    msg: &OutboundMessage,
) -> NodeResult<SendResult> {
    let mut attempt = 0;
    loop {
        attempt += 1;
        match sender.send(ctx, msg.clone()).await {
            Ok(result) => return Ok(result),
            Err(err) => {
                let retryable = err.retryable;
                let backoff_ms = err.backoff_ms;
                if retryable && attempt < MAX_ATTEMPTS {
                    let delay = backoff_ms
                        .map(Duration::from_millis)
                        .unwrap_or_else(|| Duration::from_secs(attempt as u64));
                    warn!(attempt, delay_ms = delay.as_millis(), "webex retry");
                    sleep(delay).await;
                    continue;
                } else {
                    return Err(err);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gsm_backpressure::{LocalBackpressureLimiter, RateLimits};
    use gsm_core::{NodeError, OutKind, make_tenant_ctx};
    use serde_json::{Value, json};
    use std::{
        collections::BTreeMap,
        sync::{Arc, Mutex},
    };
    use tokio::sync::Mutex as AsyncMutex;

    struct MockSender {
        responses: AsyncMutex<Vec<NodeResult<SendResult>>>,
        calls: AsyncMutex<usize>,
    }

    impl MockSender {
        fn new(responses: Vec<NodeResult<SendResult>>) -> Self {
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
    impl EgressSender for MockSender {
        async fn send(&self, _ctx: &TenantCtx, _msg: OutboundMessage) -> NodeResult<SendResult> {
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

        async fn ack_with(&self, _kind: AckKind) -> Result<()> {
            *self.acked.lock().unwrap() = true;
            Ok(())
        }
    }

    fn sample_out() -> OutMessage {
        let mut meta = BTreeMap::new();
        meta.insert("source_msg_id".into(), Value::String("mid-1".into()));
        OutMessage {
            ctx: make_tenant_ctx("acme".into(), None, None),
            tenant: "acme".into(),
            platform: Platform::Webex,
            chat_id: "room-42".into(),
            thread_id: None,
            kind: OutKind::Text,
            text: Some("hi".into()),
            message_card: None,
            adaptive_card: None,
            meta,
        }
    }

    fn limiter() -> Arc<LocalBackpressureLimiter> {
        let limits = Arc::new(RateLimits::from_env());
        Arc::new(LocalBackpressureLimiter::new(limits))
    }

    #[tokio::test]
    async fn retries_on_rate_limit_then_succeeds() {
        let retry_err = NodeError::new("webex_send_failed", "limit").with_retry(Some(1));
        let sender = MockSender::new(vec![Err(retry_err), Ok(SendResult::default())]);
        let out = sample_out();
        let outbound = OutboundMessage {
            channel: Some(out.chat_id.clone()),
            text: out.text.clone(),
            payload: Some(json!({"roomId": out.chat_id})),
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

        handle_message("acme", lim.as_ref(), &sender, &dlq, None, &message)
            .await
            .unwrap();

        assert!(message.acked());
        assert!(dlq.entries.lock().unwrap().is_empty());
        assert_eq!(sender.call_count().await, 1);
    }

    #[tokio::test]
    async fn handle_message_dlq_on_failure() {
        let sender = MockSender::new(vec![Err(NodeError::new("webex_send_failed", "bad"))]);
        let dlq = MockDlq::new();
        let lim = limiter();
        let out = sample_out();
        let message = MockMessage::new(&out);

        handle_message("acme", lim.as_ref(), &sender, &dlq, None, &message)
            .await
            .unwrap();

        assert!(message.acked());
        {
            let entries = dlq.entries.lock().unwrap();
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].code, "webex_send_failed");
        }
        assert_eq!(sender.call_count().await, 1);
    }
}
