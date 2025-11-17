//! Microsoft Teams egress adapter. Listens on NATS, renders payloads, and posts
//! them via the Graph API using per-tenant credentials resolved from secrets.

use anyhow::{Result, anyhow};
use async_nats::jetstream::{self, AckKind};
use async_trait::async_trait;
use futures::StreamExt;
use gsm_backpressure::BackpressureLimiter;
use gsm_core::egress::{EgressSender, OutboundMessage, SendResult};
use gsm_core::messaging_card::ensure_oauth_start_url;
use gsm_core::messaging_card::{MessageCardEngine, MessageCardKind, RenderSpec};
use gsm_core::oauth::{OauthClient, ReqwestTransport};
use gsm_core::platforms::teams::TeamsSender;
use gsm_core::prelude::DefaultResolver;
use gsm_core::telemetry::{MessageContext, install as init_telemetry};
use gsm_core::{AdaptiveMessageCard, NodeError, OutKind, OutMessage, Platform, TenantCtx};
use gsm_dlq::{DlqError, DlqPublisher};
use gsm_egress_common::{
    egress::bootstrap,
    telemetry::{
        AuthRenderMode, context_from_out, record_auth_card_render, record_egress_success,
        start_acquire_span, start_send_span,
    },
};
use gsm_translator::teams::to_teams_adaptive;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::sleep;
use tracing::{Instrument, Level, event, warn};

const MAX_ATTEMPTS: usize = 3;

#[tokio::main]
async fn main() -> Result<()> {
    init_telemetry("greentic-messaging")?;
    let nats_url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
    let tenant = std::env::var("TENANT").unwrap_or_else(|_| "acme".into());
    #[cfg(feature = "mock-http")]
    // Mock the Microsoft Graph endpoints during local and CI runs.
    let auth_base = Some("mock://auth".into());
    #[cfg(not(feature = "mock-http"))]
    let auth_base = std::env::var("MS_GRAPH_AUTH_BASE").ok();
    #[cfg(feature = "mock-http")]
    // Pair the auth mock with a fake Graph API host.
    let api_base = Some("mock://graph".into());
    #[cfg(not(feature = "mock-http"))]
    let api_base = std::env::var("MS_GRAPH_API_BASE").ok();

    let queue = bootstrap(&nats_url, &tenant, Platform::Teams.as_str()).await?;
    tracing::info!(
        stream = %queue.stream,
        consumer = %queue.consumer,
        "egress-teams consuming from JetStream"
    );

    let dlq = Arc::new(DlqPublisher::new("egress", queue.client()).await?);
    let limiter = queue.limiter.clone();
    let resolver = Arc::new(DefaultResolver::new().await?);
    let sender = Arc::new(TeamsSender::new(
        reqwest::Client::new(),
        resolver,
        auth_base,
        api_base,
    ));

    let card_engine = Arc::new(MessageCardEngine::bootstrap());
    let oauth_client = match std::env::var("OAUTH_BASE_URL") {
        Ok(_) => match OauthClient::from_env(reqwest::Client::new()) {
            Ok(client) => {
                tracing::info!("OAUTH_BASE_URL detected; Teams OAuth builder enabled");
                Some(Arc::new(client))
            }
            Err(err) => {
                tracing::warn!(error = %err, "failed to initialize OAuth client");
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
                tracing::error!(error = %err, "jetstream message error");
                continue;
            }
        };

        if let Err(err) = handle_message(
            limiter.as_ref(),
            sender.as_ref(),
            dlq.as_ref(),
            card_engine.as_ref(),
            oauth_client.as_deref(),
            &msg,
        )
        .await
        {
            tracing::error!(error = %err, "failed to process teams message");
            if let Err(nak_err) = msg.ack_with(AckKind::Nak(None)).await {
                tracing::error!(error = %nak_err, "failed to nack message");
            }
        }
    }

    Ok(())
}

async fn handle_message<S, D, M>(
    limiter: &dyn BackpressureLimiter,
    sender: &S,
    dlq: &D,
    card_engine: &MessageCardEngine,
    oauth_client: Option<&OauthClient<ReqwestTransport>>,
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

    if out.platform != Platform::Teams {
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
    event!(
        Level::INFO,
        tenant = %ctx.labels.tenant,
        platform = "teams",
        msg_id = %msg_id,
        acquired = true,
        "backpressure permit acquired"
    );

    let send_span = start_send_span(&ctx);
    let start_time = Instant::now();

    let outbound = match build_outbound(&out, &ctx, card_engine, oauth_client).await {
        Ok(msg) => msg,
        Err(err) => {
            tracing::warn!(error = %err, "teams translation failed");
            msg.ack_with(AckKind::Nak(None)).await?;
            return Ok(());
        }
    };

    let result = {
        let _guard = send_span.enter();
        send_with_retries(sender, &out.ctx, outbound.clone()).await
    };
    match result {
        Ok(send_result) => {
            msg.ack().await?;
            let elapsed_ms = start_time.elapsed().as_secs_f64() * 1000.0;
            record_egress_success(&ctx, elapsed_ms);
            tracing::info!(
                chat_id = %out.chat_id,
                message_id = ?send_result.message_id,
                "teams message sent"
            );
        }
        Err(err) => {
            tracing::warn!(error = %err, "teams send failed");
            if err.retryable {
                msg.ack_with(AckKind::Nak(None)).await?;
            } else {
                dlq.publish_dlq(
                    &out.tenant,
                    out.platform.as_str(),
                    &msg_id,
                    DlqError {
                        code: "E_SEND".into(),
                        message: err.to_string(),
                        stage: None,
                    },
                    &out,
                )
                .await?;
                msg.ack().await?;
            }
        }
    }

    Ok(())
}

async fn build_outbound(
    out: &OutMessage,
    ctx: &MessageContext,
    engine: &MessageCardEngine,
    oauth_client: Option<&OauthClient<ReqwestTransport>>,
) -> Result<OutboundMessage, anyhow::Error> {
    let channel = out.chat_id.clone();

    if let Some(card) = &out.adaptive_card {
        match render_adaptive_card(out, ctx, card, engine, oauth_client).await {
            Ok(payload) => {
                return Ok(OutboundMessage {
                    channel: Some(channel),
                    text: out.text.clone(),
                    payload: Some(payload),
                });
            }
            Err(err) => {
                warn!(error = %err, "failed to render adaptive MessageCard; falling back to legacy payload");
            }
        }
    }

    match out.kind {
        OutKind::Text => Ok(OutboundMessage {
            channel: Some(channel),
            text: Some(out.text.clone().unwrap_or_default()),
            payload: None,
        }),
        OutKind::Card => {
            let card = out
                .message_card
                .as_ref()
                .ok_or_else(|| anyhow!("missing card"))?;
            let adaptive = to_teams_adaptive(card, out)?;
            Ok(OutboundMessage {
                channel: Some(channel),
                text: out.text.clone(),
                payload: Some(adaptive),
            })
        }
    }
}

async fn render_adaptive_card(
    out: &OutMessage,
    ctx: &MessageContext,
    card: &AdaptiveMessageCard,
    engine: &MessageCardEngine,
    oauth_client: Option<&OauthClient<ReqwestTransport>>,
) -> Result<serde_json::Value> {
    let mut working = card.clone();
    if matches!(working.kind, MessageCardKind::Oauth) {
        let has_url = working
            .oauth
            .as_ref()
            .and_then(|oauth| oauth.start_url.as_ref())
            .is_some();
        if let Some(client) = oauth_client {
            ensure_oauth_start_url(&mut working, &out.ctx, client, None).await?;
        } else if !has_url {
            return Err(anyhow!(
                "oauth card missing start_url and OAuth builder not configured"
            ));
        }
    }

    let spec = engine
        .render_spec(&working)
        .map_err(|err| anyhow!("message card normalization failed: {err}"))?;
    let snapshot = engine
        .render_snapshot_tracked("teams", &spec)
        .ok_or_else(|| anyhow!("teams renderer unavailable"))?;

    if matches!(spec, RenderSpec::Auth(_))
        && let Some(oauth) = working.oauth.as_ref()
    {
        let mode = if snapshot.ir.is_none() {
            AuthRenderMode::Native
        } else {
            AuthRenderMode::Downgrade
        };
        let team = out.ctx.team.as_ref().map(|team| team.as_ref());
        record_auth_card_render(
            ctx,
            oauth.provider.as_str(),
            mode,
            oauth.connection_name.as_deref(),
            oauth.start_url.as_deref(),
            team,
        );
    }

    let payload = snapshot.output.payload;
    Ok(payload)
}

async fn send_with_retries<S>(
    sender: &S,
    ctx: &TenantCtx,
    msg: OutboundMessage,
) -> Result<SendResult, NodeError>
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
                    tracing::warn!(attempt, delay_ms = delay.as_millis(), "teams retry");
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
trait DlqSink {
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
            .map_err(|err| anyhow::anyhow!(err))
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
        self.ack().await.map_err(|err| anyhow::anyhow!(err))
    }

    async fn ack_with(&self, kind: AckKind) -> Result<()> {
        self.ack_with(kind)
            .await
            .map_err(|err| anyhow::anyhow!(err))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn build_outbound_text_uses_message_text() {
        let mut out = sample_out(OutKind::Text);
        out.text = Some("hello".into());
        let engine = MessageCardEngine::bootstrap();
        let ctx = context_from_out(&out);
        let outbound = build_outbound(&out, &ctx, &engine, None).await.unwrap();
        assert_eq!(outbound.channel.as_deref(), Some("chat-1"));
        assert_eq!(outbound.text.as_deref(), Some("hello"));
        assert!(outbound.payload.is_none());
    }

    #[tokio::test]
    async fn build_outbound_card_wraps_payload() {
        let mut out = sample_out(OutKind::Card);
        out.message_card = Some(gsm_core::MessageCard {
            title: Some("Title".into()),
            body: vec![gsm_core::CardBlock::Text {
                text: "Body".into(),
                markdown: false,
            }],
            actions: vec![],
        });
        let engine = MessageCardEngine::bootstrap();
        let ctx = context_from_out(&out);
        let outbound = build_outbound(&out, &ctx, &engine, None).await.unwrap();
        assert!(outbound.payload.is_some());
    }

    fn sample_out(kind: OutKind) -> OutMessage {
        OutMessage {
            ctx: gsm_core::make_tenant_ctx("acme".into(), Some("support".into()), None),
            tenant: "acme".into(),
            platform: Platform::Teams,
            chat_id: "chat-1".into(),
            thread_id: None,
            kind,
            text: None,
            message_card: None,
            adaptive_card: None,
            meta: Default::default(),
        }
    }
}
