//! Telegram egress adapter that translates `OutMessage` payloads into Bot API requests.

use anyhow::Result;
use futures::StreamExt;
use gsm_core::{OutMessage, Platform};
use gsm_translator::{TelegramTranslator, Translator};
use serde_json::Value;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_target(false)
        .init();
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
    let subject = format!("greentic.msg.out.{}.telegram.>", tenant);
    let mut sub = nats.subscribe(subject.clone()).await?;
    tracing::info!("egress-telegram subscribed to {subject}");

    while let Some(msg) = sub.next().await {
        let out: OutMessage = match serde_json::from_slice(&msg.payload) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("bad out msg: {e}");
                continue;
            }
        };
        if out.platform != Platform::Telegram {
            continue;
        }

        match translator.to_platform(&out) {
            Ok(mut payloads) => {
                for payload in payloads.iter_mut() {
                    enrich_payload(payload, &out);
                    if let Err(err) =
                        send_payload(&client, &api_base, &bot_token, payload.clone()).await
                    {
                        tracing::warn!("telegram send failed: {err}");
                    }
                }
            }
            Err(e) => tracing::warn!("translator error: {e}"),
        }
    }

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

async fn send_payload(
    client: &reqwest::Client,
    api_base: &str,
    bot_token: &str,
    payload: Value,
) -> Result<()> {
    let url = build_api_url(api_base, bot_token);
    let res = client.post(url).json(&payload).send().await?;
    if !res.status().is_success() {
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        anyhow::bail!("telegram api err {}: {}", status, text);
    }
    Ok(())
}

fn build_api_url(api_base: &str, bot_token: &str) -> String {
    format!(
        "{}/bot{}/sendMessage",
        api_base.trim_end_matches('/'),
        bot_token
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
        let url = build_api_url("https://api.telegram.org/", "token-123");
        assert_eq!(url, "https://api.telegram.org/bottoken-123/sendMessage");
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
