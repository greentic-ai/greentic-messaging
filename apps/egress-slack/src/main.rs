//! Slack egress adapter that converts normalized messages into Slack API calls.

use anyhow::{Context, Result};
use async_nats::jetstream::AckKind;
use futures::StreamExt;
use gsm_backpressure::BackpressureLimiter;
use gsm_core::{OutMessage, Platform};
use gsm_dlq::{DlqError, DlqPublisher};
use gsm_egress_common::egress::bootstrap;
use gsm_telemetry::{
    init_telemetry, record_counter, record_histogram, MessageContext, TelemetryConfig,
};
use gsm_translator::slack::to_slack_payloads;
use serde_json::Value;
use std::time::Instant;
use tracing::{event, Instrument, Level};

#[tokio::main]
async fn main() -> Result<()> {
    let telemetry = TelemetryConfig::from_env("gsm-egress-slack", env!("CARGO_PKG_VERSION"));
    init_telemetry(telemetry)?;

    let nats_url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
    let tenant = std::env::var("TENANT").unwrap_or_else(|_| "acme".into());
    let bot_token = std::env::var("SLACK_BOT_TOKEN")
        .context("SLACK_BOT_TOKEN environment variable required")?;

    let queue = bootstrap(&nats_url, &tenant, Platform::Slack.as_str()).await?;
    tracing::info!(
        stream = %queue.stream,
        consumer = %queue.consumer,
        "egress-slack consuming from JetStream"
    );

    let dlq = DlqPublisher::new("egress", queue.client()).await?;

    let http = reqwest::Client::new();
    let mut messages = queue.messages;
    let limiter = queue.limiter;

    while let Some(next) = messages.next().await {
        let msg = match next {
            Ok(msg) => msg,
            Err(err) => {
                tracing::error!("jetstream message error: {err}");
                continue;
            }
        };

        let out: OutMessage = match serde_json::from_slice(msg.payload.as_ref()) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("bad out msg: {e}");
                let _ = msg.ack().await;
                continue;
            }
        };
        if out.platform != Platform::Slack {
            if let Err(err) = msg.ack().await {
                tracing::error!("ack failed: {err}");
            }
            continue;
        }

        let msg_ctx = MessageContext::from_out(&out);
        let msg_id = msg_ctx
            .labels
            .msg_id
            .clone()
            .unwrap_or_else(|| out.message_id());
        let span = tracing::info_span!(
            "egress.acquire_permit",
            tenant = %msg_ctx.labels.tenant,
            platform = "slack",
            chat_id = %out.chat_id,
            msg_id = %msg_id
        );
        let permit = match limiter.acquire(&out.tenant).instrument(span).await {
            Ok(p) => p,
            Err(err) => {
                tracing::error!(
                    error = %err,
                    tenant = %out.tenant,
                    platform = "slack",
                    "failed to acquire backpressure permit"
                );
                let _ = msg.ack_with(AckKind::Nak(None)).await;
                continue;
            }
        };
        event!(
            Level::INFO,
            tenant = %out.tenant,
            platform = "slack",
            msg_id = %msg_id,
            acquired = true,
            "backpressure permit acquired"
        );

        let translate_span = tracing::info_span!(
            "translate.run",
            tenant = %msg_ctx.labels.tenant,
            platform = %out.platform.as_str(),
            chat_id = %out.chat_id,
            msg_id = %msg_id
        );
        let payloads = {
            let _guard = translate_span.enter();
            match to_slack_payloads(&out) {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!("slack translation failed: {e}");
                    drop(permit);
                    let _ = msg.ack_with(AckKind::Nak(None)).await;
                    continue;
                }
            }
        };

        let mut had_error = false;
        let mut last_error: Option<String> = None;
        {
            let send_span = tracing::info_span!(
                "egress.send",
                tenant = %msg_ctx.labels.tenant,
                platform = %out.platform.as_str(),
                chat_id = %out.chat_id,
                msg_id = %msg_id
            );
            let _guard = send_span.enter();
            for mut payload in payloads {
                merge_channel_info(&mut payload, &out);
                let send_start = Instant::now();
                match http
                    .post("https://slack.com/api/chat.postMessage")
                    .bearer_auth(&bot_token)
                    .json(&payload)
                    .send()
                    .await
                {
                    Ok(response) if response.status().is_success() => {
                        tracing::debug!("slack message posted: {}", out.chat_id);
                    }
                    Ok(response) => {
                        let status = response.status();
                        let text = response.text().await.unwrap_or_default();
                        tracing::warn!("slack api err {}: {}", status, text);
                        had_error = true;
                        last_error = Some(format!("HTTP {}: {}", status, text));
                    }
                    Err(error) => {
                        tracing::warn!("slack http error: {error}");
                        had_error = true;
                        last_error = Some(error.to_string());
                    }
                }
                let elapsed = send_start.elapsed().as_secs_f64() * 1000.0;
                record_histogram("egress_send_latency_ms", elapsed, &msg_ctx.labels);
            }
        }
        drop(permit);

        if had_error {
            if let Some(err_msg) = last_error {
                if let Err(err) = dlq
                    .publish(
                        &out.tenant,
                        out.platform.as_str(),
                        &msg_id,
                        1,
                        DlqError {
                            code: "E_SEND".into(),
                            message: err_msg,
                            stage: None,
                        },
                        &out,
                    )
                    .await
                {
                    tracing::error!("failed to publish dlq entry: {err}");
                }
            }
            if let Err(err) = msg.ack_with(AckKind::Nak(None)).await {
                tracing::error!("nak failed: {err}");
            }
        } else if let Err(err) = msg.ack().await {
            tracing::error!("ack failed: {err}");
        } else {
            record_counter("messages_egressed", 1, &msg_ctx.labels);
        }
    }

    Ok(())
}

fn merge_channel_info(payload: &mut Value, out: &OutMessage) {
    if let Some(obj) = payload.as_object_mut() {
        obj.insert("channel".into(), out.chat_id.clone().into());
        if let Some(thread) = &out.thread_id {
            obj.insert("thread_ts".into(), thread.clone().into());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gsm_core::{OutKind, OutMessage, Platform};
    use serde_json::json;

    fn sample_out(thread_id: Option<&str>) -> OutMessage {
        OutMessage {
            tenant: "acme".into(),
            platform: Platform::Slack,
            chat_id: "C123".into(),
            thread_id: thread_id.map(|s| s.into()),
            kind: OutKind::Text,
            text: Some("hello".into()),
            message_card: None,
            meta: Default::default(),
        }
    }

    #[test]
    fn merge_channel_info_sets_channel_and_thread() {
        let mut payload = json!({ "text": "ok" });
        let out = sample_out(Some("1711111111.000100"));
        merge_channel_info(&mut payload, &out);
        assert_eq!(payload["channel"], "C123");
        assert_eq!(payload["thread_ts"], "1711111111.000100");
    }

    #[test]
    fn merge_channel_info_without_thread() {
        let mut payload = json!({ "text": "ok" });
        let out = sample_out(None);
        merge_channel_info(&mut payload, &out);
        assert_eq!(payload["channel"], "C123");
        assert!(!payload.as_object().unwrap().contains_key("thread_ts"));
    }

    #[test]
    fn merge_channel_info_overrides_existing_fields() {
        let mut payload = json!({
            "channel": "old",
            "thread_ts": "old-thread",
            "text": "hello"
        });
        let out = sample_out(Some("1710000000.123456"));
        merge_channel_info(&mut payload, &out);
        assert_eq!(payload["channel"], "C123");
        assert_eq!(payload["thread_ts"], "1710000000.123456");
        assert_eq!(payload["text"], "hello");
    }
}
