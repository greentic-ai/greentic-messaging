#![cfg(feature = "e2e")]

use anyhow::{Context, Result, anyhow};
use gsm_core::{CardBlock, MessageCard, OutKind, OutMessage, Platform, make_tenant_ctx};
use gsm_testutil::e2e::assertions::{assert_has_block_type, message_contains_text};
use gsm_testutil::visual;
use gsm_translator::slack::to_slack_payloads;
use reqwest::header::RETRY_AFTER;
use reqwest::{Client, StatusCode};
use serde_json::{Value, json};
use std::error::Error;
use std::future::Future;
use tokio::time::{Duration, sleep};

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

async fn request_with_retry<F, Fut>(mut op: F) -> Result<reqwest::Response, reqwest::Error>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<reqwest::Response, reqwest::Error>>,
{
    const MAX_ATTEMPTS: usize = 2;
    let mut delay = Duration::from_millis(500);

    for attempt in 0..MAX_ATTEMPTS {
        match op().await {
            Ok(resp) => {
                if resp.status() == StatusCode::TOO_MANY_REQUESTS && attempt + 1 < MAX_ATTEMPTS {
                    let wait = retry_after_delay(&resp).unwrap_or(delay);
                    drop(resp);
                    sleep(wait).await;
                    delay *= 2;
                    continue;
                }
                return Ok(resp);
            }
            Err(err) => {
                if attempt + 1 < MAX_ATTEMPTS && (err.is_connect() || err.is_timeout()) {
                    sleep(delay).await;
                    delay *= 2;
                    continue;
                }
                return Err(err);
            }
        }
    }

    unreachable!("request_with_retry exhausted attempts without returning");
}

fn retry_after_delay(resp: &reqwest::Response) -> Option<Duration> {
    resp.headers()
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_secs)
}

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

        let post_url = "https://slack.com/api/chat.postMessage".to_string();
        let response = request_with_retry(|| {
            let client = client.clone();
            let token = token.clone();
            let body = body.clone();
            let post_url = post_url.clone();
            async move {
                client
                    .post(&post_url)
                    .bearer_auth(&token)
                    .json(&body)
                    .send()
                    .await
            }
        })
        .await
        .map_err(handle_reqwest_error)?;

        let status = response.status();
        let body: Value = response
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

    let history_url = "https://slack.com/api/conversations.history".to_string();
    let history = request_with_retry(|| {
        let client = client.clone();
        let token = token.clone();
        let channel = channel.clone();
        let ts = ts.clone();
        let history_url = history_url.clone();
        async move {
            client
                .get(&history_url)
                .bearer_auth(&token)
                .query(&[
                    ("channel", channel.as_str()),
                    ("latest", ts.as_str()),
                    ("inclusive", "true"),
                    ("limit", "1"),
                ])
                .send()
                .await
        }
    })
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

    if let Err(err) = delete_slack_message(&client, &token, &channel, &ts).await {
        eprintln!("failed to delete slack message: {err:#}");
    }

    Ok(())
}

async fn delete_slack_message(client: &Client, token: &str, channel: &str, ts: &str) -> Result<()> {
    let delete_url = "https://slack.com/api/chat.delete".to_string();
    let response = request_with_retry(|| {
        let client = client.clone();
        let token = token.to_string();
        let channel = channel.to_string();
        let ts = ts.to_string();
        let delete_url = delete_url.clone();
        async move {
            client
                .post(&delete_url)
                .bearer_auth(&token)
                .json(&json!({
                    "channel": channel,
                    "ts": ts,
                }))
                .send()
                .await
        }
    })
    .await
    .map_err(handle_reqwest_error)?;

    let body: Value = response.json().await.context("chat.delete decode failed")?;

    if !body.get("ok").and_then(Value::as_bool).unwrap_or(false) {
        return Err(anyhow!("chat.delete returned error: {body}"));
    }

    Ok(())
}

async fn fetch_permalink(
    client: &Client,
    token: &str,
    channel: &str,
    ts: &str,
) -> Result<Option<String>> {
    let permalink_url = "https://slack.com/api/chat.getPermalink".to_string();
    let response = request_with_retry(|| {
        let client = client.clone();
        let token = token.to_string();
        let channel = channel.to_string();
        let ts = ts.to_string();
        let permalink_url = permalink_url.clone();
        async move {
            client
                .get(&permalink_url)
                .bearer_auth(&token)
                .query(&[("channel", channel.as_str()), ("message_ts", ts.as_str())])
                .send()
                .await
        }
    })
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
