//! WhatsApp egress adapter. Sends text messages when within the 24-hour session
//! window and falls back to approved templates when required.

use anyhow::{anyhow, Result};
use async_nats::jetstream::AckKind;
use futures::StreamExt;
use gsm_backpressure::BackpressureLimiter;
use gsm_core::{OutKind, OutMessage, Platform};
use gsm_dlq::{DlqError, DlqPublisher};
use gsm_egress_common::{
    egress::bootstrap,
    telemetry::{context_from_out, record_egress_success, start_acquire_span, start_send_span},
};
use gsm_telemetry::{init_telemetry, TelemetryConfig};
use gsm_translator::secure_action_url;
use serde_json::json;
use std::time::Instant;
use time::{Duration, OffsetDateTime};
use tracing::{event, Instrument, Level};

const SESSION_WINDOW_HOURS: i64 = 24;

#[derive(Clone)]
struct AppConfig {
    phone_id: String,
    token: String,
    template_name: String,
    template_lang: String,
    api_base: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let telemetry = TelemetryConfig::from_env("gsm-egress-whatsapp", env!("CARGO_PKG_VERSION"));
    init_telemetry(telemetry)?;

    let nats_url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
    let tenant = std::env::var("TENANT").unwrap_or_else(|_| "acme".into());
    let phone_id = std::env::var("WA_PHONE_ID").expect("WA_PHONE_ID required");
    let token = std::env::var("WA_USER_TOKEN").expect("WA_USER_TOKEN required");
    let template_name =
        std::env::var("WA_TEMPLATE_NAME").unwrap_or_else(|_| "weather_update".into());
    let template_lang = std::env::var("WA_TEMPLATE_LANG").unwrap_or_else(|_| "en".into());
    let api_base =
        std::env::var("WA_API_BASE").unwrap_or_else(|_| "https://graph.facebook.com".into());

    let config = AppConfig {
        phone_id,
        token,
        template_name,
        template_lang,
        api_base,
    };

    let queue = bootstrap(&nats_url, &tenant, Platform::WhatsApp.as_str()).await?;
    tracing::info!(
        stream = %queue.stream,
        consumer = %queue.consumer,
        "egress-whatsapp consuming from JetStream"
    );

    let client = queue.client();
    let mut messages = queue.messages;
    let limiter = queue.limiter;
    let dlq = DlqPublisher::new("egress", client).await?;
    let http = reqwest::Client::new();

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

        if out.platform != Platform::WhatsApp {
            if let Err(err) = msg.ack().await {
                tracing::error!("ack failed: {err}");
            }
            continue;
        }

        let ctx = context_from_out(&out);
        let msg_id = ctx
            .labels
            .msg_id
            .clone()
            .unwrap_or_else(|| out.message_id());
        let acquire_span = start_acquire_span(&ctx);
        let permit = match limiter.acquire(&out.tenant).instrument(acquire_span).await {
            Ok(p) => p,
            Err(err) => {
                tracing::error!(
                    error = %err,
                    tenant = %ctx.labels.tenant,
                    platform = "whatsapp",
                    "failed to acquire backpressure permit"
                );
                let _ = msg.ack_with(AckKind::Nak(None)).await;
                continue;
            }
        };
        event!(
            Level::INFO,
            tenant = %ctx.labels.tenant,
            platform = "whatsapp",
            msg_id = %msg_id,
            acquired = true,
            "backpressure permit acquired"
        );

        let send_start = Instant::now();
        let send_span = start_send_span(&ctx);
        let result = dispatch_message(&http, &config, &out)
            .instrument(send_span)
            .await;
        drop(permit);
        let elapsed_ms = send_start.elapsed().as_secs_f64() * 1000.0;

        match result {
            Ok(()) => {
                if let Err(err) = msg.ack().await {
                    tracing::error!("ack failed: {err}");
                }
                record_egress_success(&ctx, elapsed_ms);
            }
            Err(e) => {
                tracing::warn!("failed to send whatsapp message: {e}");
                if let Err(err) = dlq
                    .publish(
                        &out.tenant,
                        out.platform.as_str(),
                        &msg_id,
                        1,
                        DlqError {
                            code: "E_SEND".into(),
                            message: e.to_string(),
                            stage: None,
                        },
                        &out,
                    )
                    .await
                {
                    tracing::error!("failed to publish dlq entry: {err}");
                }
                if let Err(err) = msg.ack_with(AckKind::Nak(None)).await {
                    tracing::error!("nak failed: {err}");
                }
            }
        }
    }

    Ok(())
}

async fn dispatch_message(http: &reqwest::Client, cfg: &AppConfig, out: &OutMessage) -> Result<()> {
    let chat_id = out.chat_id.clone();
    let msg_id = out.message_id();

    enum Dispatch {
        Text { text: String },
        Fallback { text: String },
    }

    let decision = {
        let translate_span = tracing::info_span!(
            "translate.run",
            tenant = %out.tenant,
            platform = %out.platform.as_str(),
            chat_id = %chat_id,
            msg_id = %msg_id
        );
        let _guard = translate_span.enter();
        match out.kind {
            OutKind::Text => {
                let text = out.text.clone().unwrap_or_default();
                if within_session_window(out) {
                    Dispatch::Text { text }
                } else {
                    tracing::info!("session window expired; sending template fallback");
                    Dispatch::Fallback { text }
                }
            }
            OutKind::Card => Dispatch::Fallback {
                text: String::new(),
            },
        }
    };

    match decision {
        Dispatch::Text { text } => send_text(http, cfg, &chat_id, &text).await,
        Dispatch::Fallback { text } => send_card_fallback(http, cfg, out, &chat_id, &text).await,
    }
}

fn within_session_window(out: &OutMessage) -> bool {
    let last_interacted = out
        .meta
        .get("wa_last_interaction")
        .and_then(|v| v.as_str())
        .and_then(|s| OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok())
        .unwrap_or_else(|| OffsetDateTime::now_utc());

    OffsetDateTime::now_utc() - last_interacted <= Duration::hours(SESSION_WINDOW_HOURS)
}

async fn send_card_fallback(
    http: &reqwest::Client,
    cfg: &AppConfig,
    out: &OutMessage,
    chat_id: &str,
    text: &str,
) -> Result<()> {
    let title = out
        .message_card
        .as_ref()
        .and_then(|c| c.title.clone())
        .unwrap_or_else(|| text.to_string());
    let mut vars = Vec::new();
    if !title.is_empty() {
        vars.push(title.as_str());
    }
    let fallback_raw =
        std::env::var("WA_FALLBACK_URL").unwrap_or_else(|_| "https://app.greentic.ai".into());
    let fallback_url = secure_action_url(out, "fallback", &fallback_raw);
    vars.push(fallback_url.as_str());

    match send_template(http, cfg, chat_id, vars.as_slice()).await {
        Ok(()) => Ok(()),
        Err(e) => {
            tracing::warn!("template send failed, falling back to text: {e}");
            let fallback_text = if text.is_empty() {
                format!("View details: {}", fallback_url)
            } else {
                format!("{} — {}", text, fallback_url)
            };
            send_text(http, cfg, chat_id, &fallback_text).await
        }
    }
}

async fn send_text(http: &reqwest::Client, cfg: &AppConfig, to: &str, body: &str) -> Result<()> {
    let url = format!(
        "{}/v19.0/{}/messages",
        cfg.api_base.trim_end_matches('/'),
        cfg.phone_id
    );
    let payload = json!({
        "messaging_product": "whatsapp",
        "to": to,
        "type": "text",
        "text": { "preview_url": true, "body": body }
    });
    let response = http
        .post(url)
        .bearer_auth(&cfg.token)
        .json(&payload)
        .send()
        .await?;
    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        tracing::warn!("wa text err: {} {}", status, text);
    }
    Ok(())
}

async fn send_template(
    http: &reqwest::Client,
    cfg: &AppConfig,
    to: &str,
    variables: &[&str],
) -> Result<()> {
    let url = format!(
        "{}/v19.0/{}/messages",
        cfg.api_base.trim_end_matches('/'),
        cfg.phone_id
    );
    let body = json!({
        "messaging_product": "whatsapp",
        "to": to,
        "type": "template",
        "template": {
          "name": cfg.template_name,
          "language": { "code": cfg.template_lang },
          "components": [
            {
              "type": "body",
              "parameters": variables.iter().map(|v| json!({
                "type": "text",
                "text": v
              })).collect::<Vec<_>>()
            }
          ]
        }
    });

    let response = http
        .post(url)
        .bearer_auth(&cfg.token)
        .json(&body)
        .send()
        .await?;
    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        return Err(anyhow!("wa template err: {} {}", status, text));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_message(timestamp_offset_hours: i64) -> OutMessage {
        let last = OffsetDateTime::now_utc() - Duration::hours(timestamp_offset_hours);
        let mut meta = serde_json::Map::new();
        meta.insert(
            "wa_last_interaction".into(),
            json!(last
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap()),
        );
        OutMessage {
            tenant: "acme".into(),
            platform: Platform::WhatsApp,
            chat_id: "12345".into(),
            thread_id: None,
            kind: OutKind::Text,
            text: Some("Hello".into()),
            message_card: None,
            meta: meta.into_iter().collect(),
        }
    }

    #[test]
    fn within_session_window_true() {
        let out = sample_message(1);
        assert!(within_session_window(&out));
    }

    #[test]
    fn within_session_window_false() {
        let out = sample_message(48);
        assert!(!within_session_window(&out));
    }

    #[test]
    fn send_template_builds_body() {
        let cfg = AppConfig {
            phone_id: "123".into(),
            token: "token".into(),
            template_name: "weather".into(),
            template_lang: "en".into(),
            api_base: "https://graph.facebook.com".into(),
        };
        let url = format!(
            "{}/v19.0/{}/messages",
            cfg.api_base.trim_end_matches('/'),
            cfg.phone_id
        );
        assert_eq!(url, "https://graph.facebook.com/v19.0/123/messages");
    }
}
