//! WhatsApp egress adapter. Sends text messages when within the 24-hour session
//! window and falls back to approved templates when required.

use anyhow::{anyhow, Result};
use async_nats::Client as Nats;
use futures::StreamExt;
use gsm_core::{OutKind, OutMessage, Platform};
use serde_json::json;
use time::{Duration, OffsetDateTime};
use tracing_subscriber::EnvFilter;

const SESSION_WINDOW_HOURS: i64 = 24;

#[derive(Clone)]
struct AppConfig {
    tenant: String,
    phone_id: String,
    token: String,
    template_name: String,
    template_lang: String,
    api_base: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

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
        tenant,
        phone_id,
        token,
        template_name,
        template_lang,
        api_base,
    };

    let nats = async_nats::connect(nats_url).await?;
    run(nats, config).await
}

async fn run(nats: Nats, config: AppConfig) -> Result<()> {
    let subject = format!("greentic.msg.out.{}.whatsapp.>", config.tenant);
    let mut sub = nats.subscribe(subject.clone()).await?;
    tracing::info!("egress-whatsapp subscribed to {subject}");

    let http = reqwest::Client::new();

    while let Some(msg) = sub.next().await {
        let out: OutMessage = match serde_json::from_slice(&msg.payload) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("bad out msg: {e}");
                continue;
            }
        };

        if out.platform != Platform::WhatsApp {
            continue;
        }

        if let Err(e) = dispatch_message(&http, &config, &out).await {
            tracing::warn!("failed to send whatsapp message: {e}");
        }
    }

    Ok(())
}

async fn dispatch_message(http: &reqwest::Client, cfg: &AppConfig, out: &OutMessage) -> Result<()> {
    let chat_id = out.chat_id.clone();
    match out.kind {
        OutKind::Text => {
            let text = out.text.clone().unwrap_or_default();
            if within_session_window(out) {
                send_text(http, cfg, &chat_id, &text).await
            } else {
                tracing::info!("session window expired; sending template fallback");
                send_card_fallback(http, cfg, out, &chat_id, &text).await
            }
        }
        OutKind::Card => send_card_fallback(http, cfg, out, &chat_id, "").await,
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
    let fallback_owned =
        std::env::var("WA_FALLBACK_URL").unwrap_or_else(|_| "https://app.greentic.ai".into());
    vars.push(fallback_owned.as_str());

    match send_template(http, cfg, chat_id, vars.as_slice()).await {
        Ok(()) => Ok(()),
        Err(e) => {
            tracing::warn!("template send failed, falling back to text: {e}");
            let fallback_text = if text.is_empty() {
                format!("View details: {}", vars.last().unwrap())
            } else {
                format!("{} — {}", text, vars.last().unwrap())
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
            tenant: "acme".into(),
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
