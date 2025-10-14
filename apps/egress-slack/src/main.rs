//! Slack egress adapter that converts normalized messages into Slack API calls.

use anyhow::{Context, Result};
use futures::StreamExt;
use gsm_core::{OutMessage, Platform};
use gsm_translator::slack::to_slack_payloads;
use serde_json::Value;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let nats_url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
    let tenant = std::env::var("TENANT").unwrap_or_else(|_| "acme".into());
    let bot_token = std::env::var("SLACK_BOT_TOKEN")
        .context("SLACK_BOT_TOKEN environment variable required")?;

    let nats = async_nats::connect(nats_url).await?;
    let subject = format!("greentic.msg.out.{}.slack.>", tenant);
    let mut sub = nats.subscribe(subject.clone()).await?;
    tracing::info!("egress-slack subscribed to {subject}");

    let http = reqwest::Client::new();

    while let Some(msg) = sub.next().await {
        let out: OutMessage = match serde_json::from_slice(&msg.payload) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("bad out msg: {e}");
                continue;
            }
        };
        if out.platform != Platform::Slack {
            continue;
        }

        let payloads = match to_slack_payloads(&out) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("slack translation failed: {e}");
                continue;
            }
        };

        for mut payload in payloads {
            merge_channel_info(&mut payload, &out);
            let res = http
                .post("https://slack.com/api/chat.postMessage")
                .bearer_auth(&bot_token)
                .json(&payload)
                .send()
                .await;

            match res {
                Ok(response) if response.status().is_success() => {
                    tracing::debug!("slack message posted: {}", out.chat_id);
                }
                Ok(response) => {
                    let status = response.status();
                    let text = response.text().await.unwrap_or_default();
                    tracing::warn!("slack api err {}: {}", status, text);
                }
                Err(error) => tracing::warn!("slack http error: {error}"),
            }
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
