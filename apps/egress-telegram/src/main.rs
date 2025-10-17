//! Telegram egress adapter that translates `OutMessage` payloads into Bot API requests.

use anyhow::{anyhow, Context, Result};
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
use gsm_core::{OutMessage, Platform};
use gsm_egress_common::telemetry::{
    context_from_out, record_egress_success, start_acquire_span, start_send_span,
};
use gsm_telemetry::{init_telemetry, TelemetryConfig};
use gsm_translator::{TelegramTranslator, Translator};
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
    let bot_token = std::env::var("TELEGRAM_BOT_TOKEN")
        .expect("TELEGRAM_BOT_TOKEN environment variable required");

    let translator = TelegramTranslator::new();
    let client = reqwest::Client::new();
    let api_base =
        std::env::var("TELEGRAM_API_BASE").unwrap_or_else(|_| "https://api.telegram.org".into());

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
        match process_message(&msg, &translator, &client, &api_base, &bot_token, &limiter).await {
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
    let mut stream_cfg = StreamConfig::default();
    stream_cfg.name = stream_name.clone();
    stream_cfg.subjects = vec![subject.clone()];
    stream_cfg.retention = RetentionPolicy::WorkQueue;
    stream_cfg.max_messages = -1;
    stream_cfg.max_messages_per_subject = -1;
    stream_cfg.max_bytes = -1;
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

async fn process_message(
    msg: &async_nats::jetstream::Message,
    translator: &TelegramTranslator,
    client: &reqwest::Client,
    api_base: &str,
    bot_token: &str,
    limiter: &Arc<HybridLimiter>,
) -> Result<()> {
    let out: OutMessage = serde_json::from_slice(msg.payload.as_ref())
        .context("decode OutMessage from JetStream payload")?;
    if out.platform != Platform::Telegram {
        tracing::debug!("skip non-telegram payload");
        return Ok(());
    }
    let ctx = context_from_out(&out);
    let permit = limiter
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
            match send_payload(client, api_base, bot_token, payload.clone()).await {
                Ok(()) => {}
                Err(SendError::Permanent(reason)) => {
                    tracing::warn!(
                        tenant = %out.tenant,
                        chat_id = %out.chat_id,
                        event = "telegram_egress_permanent_failure",
                        %reason,
                        "telegram permanent failure; acking message to avoid retries"
                    );
                    permanent_failure = Some(reason);
                    break;
                }
                Err(SendError::Transient(err)) => {
                    let err = err.context(format!("telegram api send chat={}", out.chat_id));
                    return Err(err);
                }
            }
        }
    }
    drop(permit);
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

enum SendError {
    Permanent(String),
    Transient(anyhow::Error),
}

async fn send_payload(
    client: &reqwest::Client,
    api_base: &str,
    bot_token: &str,
    mut payload: Value,
) -> Result<(), SendError> {
    let method = extract_method(&mut payload).map_err(SendError::Transient)?;
    let url = build_api_url(api_base, bot_token, &method);

    let res = client
        .post(url)
        .json(&payload)
        .send()
        .await
        .map_err(|err| SendError::Transient(err.into()))?;
    let status = res.status();
    if !status.is_success() {
        let text = res.text().await.unwrap_or_default();
        if status.is_client_error() {
            return Err(SendError::Permanent(format!(
                "telegram api err {}: {}",
                status, text
            )));
        } else {
            return Err(SendError::Transient(anyhow!(
                "telegram api err {}: {}",
                status,
                text
            )));
        }
    }
    Ok(())
}

fn extract_method(payload: &mut Value) -> Result<String> {
    let obj = payload
        .as_object_mut()
        .context("telegram payload must be an object")?;
    if let Some(method) = obj.remove("method") {
        let method = method
            .as_str()
            .context("telegram payload method must be a string")?;
        anyhow::ensure!(
            !method.is_empty(),
            "telegram payload method cannot be empty"
        );
        Ok(method.to_string())
    } else {
        Ok("sendMessage".into())
    }
}

fn build_api_url(api_base: &str, bot_token: &str, method: &str) -> String {
    format!(
        "{}/bot{}/{}",
        api_base.trim_end_matches('/'),
        bot_token,
        method
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use gsm_core::OutKind;
    use serde_json::json;

    fn sample_out(thread: Option<&str>) -> OutMessage {
        OutMessage {
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
    fn build_api_url_trims_slash() {
        let url = build_api_url("https://api.telegram.org/", "token-123", "sendPhoto");
        assert_eq!(url, "https://api.telegram.org/bottoken-123/sendPhoto");
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

    #[test]
    fn extract_method_defaults_to_send_message() {
        let mut payload = json!({"text": "hello"});
        let method = extract_method(&mut payload).unwrap();
        assert_eq!(method, "sendMessage");
        assert!(!payload.as_object().unwrap().contains_key("method"));
    }

    #[test]
    fn extract_method_uses_custom_method() {
        let mut payload = json!({"method": "sendPhoto", "photo": "abc"});
        let method = extract_method(&mut payload).unwrap();
        assert_eq!(method, "sendPhoto");
        assert!(!payload.as_object().unwrap().contains_key("method"));
    }
}
