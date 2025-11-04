#![cfg(feature = "e2e")]

use anyhow::{Context, Result, anyhow};
use gsm_core::{CardBlock, MessageCard, OutKind, OutMessage, Platform, make_tenant_ctx};
use gsm_testutil::e2e::assertions::{assert_has_block_type, message_contains_text};
use gsm_testutil::visual;
use gsm_translator::slack::to_slack_payloads;
use reqwest::Client;
use serde_json::{Value, json};
use std::error::Error;

const WEATHER_CARD_PATH: &str = "../cards/samples/weather.json";

#[test]
#[ignore]
fn slack_weather_card_e2e() {
    dotenvy::dotenv().ok();

    let token = match std::env::var("SLACK_BOT_TOKEN") {
        Ok(value) if !value.trim().is_empty() => value,
        _ => {
            eprintln!("skipping slack e2e: SLACK_BOT_TOKEN missing");
            return;
        }
    };

    let channel = match std::env::var("SLACK_CHANNEL_ID") {
        Ok(value) if !value.trim().is_empty() => value,
        _ => {
            eprintln!("skipping slack e2e: SLACK_CHANNEL_ID missing");
            return;
        }
    };

    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");

    if let Err(err) = runtime.block_on(run_slack_e2e(token, channel)) {
        if err.downcast_ref::<NetworkUnavailable>().is_some() {
            eprintln!("skipping slack e2e: network unavailable");
            return;
        }
        panic!("slack e2e test failed: {err:?}");
    }
}

#[derive(Debug)]
struct NetworkUnavailable;

impl std::fmt::Display for NetworkUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "network unavailable")
    }
}

impl Error for NetworkUnavailable {}

async fn run_slack_e2e(token: String, channel: String) -> Result<()> {
    let client = Client::new();

    let card_value = gsm_testutil::load_card!(WEATHER_CARD_PATH);
    let mut card: MessageCard =
        serde_json::from_value(card_value).context("invalid card fixture")?;
    card.body
        .retain(|block| !matches!(block, CardBlock::Image { .. }));

    let out = OutMessage {
        ctx: make_tenant_ctx("acme".into(), Some("services".into()), None),
        tenant: "acme".into(),
        platform: Platform::Slack,
        chat_id: channel.clone(),
        thread_id: None,
        kind: OutKind::Card,
        text: None,
        message_card: Some(card),
        meta: Default::default(),
    };

    let payloads = to_slack_payloads(&out).context("translator failed")?;
    if payloads.is_empty() {
        return Err(anyhow!("translator returned no payloads"));
    }

    let mut posted_ts: Option<String> = None;
    for payload in payloads {
        let mut body = payload;
        let obj = body
            .as_object_mut()
            .ok_or_else(|| anyhow!("payload is not an object"))?;
        obj.insert("channel".to_string(), json!(channel));

        let resp = client
            .post("https://slack.com/api/chat.postMessage")
            .bearer_auth(&token)
            .json(&body)
            .send()
            .await
            .map_err(handle_reqwest_error)?;

        let status = resp.status();
        let body: Value = resp
            .json()
            .await
            .context("chat.postMessage decode failed")?;

        if !body.get("ok").and_then(Value::as_bool).unwrap_or(false) {
            return Err(anyhow!(
                "chat.postMessage error: status {status}, body {body}"
            ));
        }

        if posted_ts.is_none() {
            posted_ts = body
                .get("ts")
                .and_then(Value::as_str)
                .map(|s| s.to_string());
        }
    }

    let ts = posted_ts.ok_or_else(|| anyhow!("Slack response missing ts"))?;

    let history = client
        .get("https://slack.com/api/conversations.history")
        .bearer_auth(&token)
        .query(&[
            ("channel", channel.as_str()),
            ("latest", ts.as_str()),
            ("inclusive", "true"),
            ("limit", "1"),
        ])
        .send()
        .await
        .map_err(handle_reqwest_error)?;

    let history_body: Value = history
        .json()
        .await
        .context("conversations.history decode failed")?;

    if !history_body
        .get("ok")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Err(anyhow!("conversations.history error: {history_body}"));
    }

    let messages = history_body
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("history payload missing messages"))?;

    let message = messages
        .first()
        .cloned()
        .ok_or_else(|| anyhow!("no messages returned"))?;

    assert_has_block_type(&message, "header");
    assert!(
        message_contains_text(&message, "Daily Weather"),
        "expected header text present"
    );
    assert!(
        message_contains_text(&message, "Detailed Forecast"),
        "expected action text present"
    );

    if let Some(permalink) = fetch_permalink(&client, &token, &channel, &ts).await? {
        if let Some(image) = visual::try_screenshot(&permalink) {
            println!("screenshot captured at {}", image.display());
        }
    }

    Ok(())
}

async fn fetch_permalink(
    client: &Client,
    token: &str,
    channel: &str,
    ts: &str,
) -> Result<Option<String>> {
    let response = client
        .get("https://slack.com/api/chat.getPermalink")
        .bearer_auth(token)
        .query(&[("channel", channel), ("message_ts", ts)])
        .send()
        .await
        .map_err(handle_reqwest_error)?;

    let body: Value = response
        .json()
        .await
        .context("chat.getPermalink decode failed")?;

    if !body.get("ok").and_then(Value::as_bool).unwrap_or(false) {
        return Ok(None);
    }

    Ok(body
        .get("permalink")
        .and_then(Value::as_str)
        .map(|s| s.to_string()))
}

fn handle_reqwest_error(err: reqwest::Error) -> anyhow::Error {
    if err.is_connect() || err.is_timeout() {
        NetworkUnavailable.into()
    } else {
        err.into()
    }
}
