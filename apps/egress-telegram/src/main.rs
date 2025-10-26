//! Telegram egress adapter that translates `OutMessage` payloads into Bot API requests.

mod sender;

use anyhow::{Context, Result};
use async_nats::jetstream::{
    consumer::{
        push::{Config as PushConfig, Messages},
        AckPolicy,
    },
    stream::{Config as StreamConfig, RetentionPolicy},
    AckKind, Context as JsContext,
};
use futures::StreamExt;
use gsm_backpressure::{BackpressureLimiter, HybridLimiter};
use gsm_core::egress::{EgressSender, OutboundMessage};
use gsm_core::prelude::{DefaultResolver, SecretsResolver};
use gsm_core::{NodeError, OutMessage, Platform};
use gsm_egress_common::telemetry::{
    context_from_out, record_egress_success, start_acquire_span, start_send_span,
};
use gsm_telemetry::{init_telemetry, TelemetryConfig};
use gsm_translator::{TelegramTranslator, Translator};
use sender::TelegramSender;
use serde_json::Value;
use std::{sync::Arc, time::Instant};
use tracing::Instrument;

#[tokio::main]
async fn main() -> Result<()> {
    let telemetry = TelemetryConfig::from_env("gsm-egress-telegram", env!("CARGO_PKG_VERSION"));
    init_telemetry(telemetry)?;
    tracing::info!("egress-telegram booting");

    let nats_url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
    let tenant = std::env::var("TENANT").unwrap_or_else(|_| "acme".into());
    let translator = TelegramTranslator::new();
    let client = reqwest::Client::new();
    let api_base =
        std::env::var("TELEGRAM_API_BASE").unwrap_or_else(|_| "https://api.telegram.org".into());

    let resolver = Arc::new(DefaultResolver::new().await?);
    let sender = Arc::new(TelegramSender::new(
        client.clone(),
        resolver,
        api_base.clone(),
    ));

    let nats = async_nats::connect(nats_url).await?;
    let js = async_nats::jetstream::new(nats.clone());
    let limiter = HybridLimiter::new(Some(&js)).await?;
    let (mut messages, stream_name, consumer_name) =
        init_consumer(&js, &tenant, Platform::Telegram.as_str()).await?;
    tracing::info!(
        stream = %stream_name,
        consumer = %consumer_name,
        "egress-telegram consuming from JetStream"
    );

    while let Some(next) = messages.next().await {
        let msg = match next {
            Ok(msg) => msg,
            Err(err) => {
                tracing::error!("jetstream message error: {err}");
                continue;
            }
        };
        match process_message(&msg, &translator, sender.as_ref(), &limiter).await {
            Ok(()) => {
                if let Err(err) = msg.ack().await {
                    tracing::error!("ack failed: {err}");
                }
            }
            Err(err) => {
                tracing::error!(error = %err, "telegram egress failed");
                if let Err(nak_err) = msg.ack_with(AckKind::Nak(None)).await {
                    tracing::error!("nak failed: {nak_err}");
                }
            }
        }
    }

    Ok(())
}

async fn init_consumer(
    js: &JsContext,
    tenant: &str,
    platform: &str,
) -> Result<(Messages, String, String)> {
    let subject = format!("greentic.msg.out.{}.{}.>", tenant, platform);
    let stream_name = format!("msg-out-{}-{}", tenant, platform);
    let stream_cfg = StreamConfig {
        name: stream_name.clone(),
        subjects: vec![subject.clone()],
        retention: RetentionPolicy::WorkQueue,
        max_messages: -1,
        max_messages_per_subject: -1,
        max_bytes: -1,
        ..Default::default()
    };
    let stream = js
        .get_or_create_stream(stream_cfg)
        .await
        .with_context(|| format!("ensure stream {stream_name}"))?;
    let deliver = format!("deliver.egress.{tenant}.{platform}");
    let consumer_name = format!("egress-{tenant}-{platform}");
    let consumer = stream
        .get_or_create_consumer(
            &consumer_name,
            PushConfig {
                durable_name: Some(consumer_name.clone()),
                deliver_subject: deliver.clone(),
                deliver_group: Some(format!("egress-{tenant}")),
                filter_subject: subject.clone(),
                ack_policy: AckPolicy::Explicit,
                max_ack_pending: 256,
                ..Default::default()
            },
        )
        .await
        .with_context(|| format!("ensure consumer {consumer_name}"))?;
    let messages = consumer
        .messages()
        .await
        .with_context(|| format!("attach consumer stream {consumer_name}"))?;
    Ok((messages, stream_name, consumer_name))
}

async fn process_message<R>(
    msg: &async_nats::jetstream::Message,
    translator: &TelegramTranslator,
    sender: &TelegramSender<R>,
    limiter: &Arc<HybridLimiter>,
) -> Result<()>
where
    R: SecretsResolver + Send + Sync,
{
    let out: OutMessage = serde_json::from_slice(msg.payload.as_ref())
        .context("decode OutMessage from JetStream payload")?;
    if out.platform != Platform::Telegram {
        tracing::debug!("skip non-telegram payload");
        return Ok(());
    }
    let ctx = context_from_out(&out);
    let _permit = limiter
        .acquire(&out.tenant)
        .instrument(start_acquire_span(&ctx))
        .await
        .context("acquire backpressure permit")?;
    let mut payloads = translator
        .to_platform(&out)
        .context("translate payload to telegram")?;
    let send_start = Instant::now();
    let mut permanent_failure: Option<String> = None;
    {
        let send_span = start_send_span(&ctx);
        let _guard = send_span.enter();
        for payload in payloads.iter_mut() {
            enrich_payload(payload, &out);
            let outbound = OutboundMessage {
                channel: Some(out.chat_id.clone()),
                text: out.text.clone(),
                payload: Some(payload.clone()),
            };
            match sender.send(&out.ctx, outbound).await {
                Ok(_) => {}
                Err(err) => {
                    let retryable = matches!(
                        &err,
                        NodeError::Fail {
                            retryable: true,
                            ..
                        }
                    );
                    let err_string = err.to_string();
                    if retryable {
                        return Err(err.into());
                    } else {
                        tracing::warn!(
                            tenant = %out.tenant,
                            chat_id = %out.chat_id,
                            event = "telegram_egress_permanent_failure",
                            error = %err_string,
                            "telegram permanent failure; acking message to avoid retries"
                        );
                        permanent_failure = Some(err_string);
                        break;
                    }
                }
            }
        }
    }
    if permanent_failure.is_some() {
        return Ok(());
    }
    let elapsed_ms = send_start.elapsed().as_secs_f64() * 1000.0;
    record_egress_success(&ctx, elapsed_ms);
    Ok(())
}

fn enrich_payload(payload: &mut Value, out: &OutMessage) {
    if let Some(obj) = payload.as_object_mut() {
        obj.insert("chat_id".into(), out.chat_id.clone().into());
        if let Some(thread) = &out.thread_id {
            obj.insert("reply_to_message_id".into(), thread.clone().into());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gsm_core::{make_tenant_ctx, OutKind};
    use serde_json::json;

    fn sample_out(thread: Option<&str>) -> OutMessage {
        OutMessage {
            ctx: make_tenant_ctx("acme".into(), None, None),
            tenant: "acme".into(),
            platform: Platform::Telegram,
            chat_id: "chat-1".into(),
            thread_id: thread.map(|s| s.into()),
            kind: OutKind::Text,
            text: Some("hello".into()),
            message_card: None,
            meta: Default::default(),
        }
    }

    #[test]
    fn enrich_payload_sets_chat_and_reply() {
        let mut payload = json!({"text": "hello"});
        let out = sample_out(Some("42"));
        enrich_payload(&mut payload, &out);
        assert_eq!(payload["chat_id"], "chat-1");
        assert_eq!(payload["reply_to_message_id"], "42");
    }

    #[test]
    fn enrich_payload_without_thread() {
        let mut payload = json!({"text": "hello"});
        let out = sample_out(None);
        enrich_payload(&mut payload, &out);
        assert_eq!(payload["chat_id"], "chat-1");
        assert!(!payload
            .as_object()
            .unwrap()
            .contains_key("reply_to_message_id"));
    }
}
