#![cfg(feature = "e2e")]

use anyhow::{Context, Result, anyhow, ensure};
use gsm_core::{CardBlock, MessageCard, OutKind, OutMessage, Platform, make_tenant_ctx};
use gsm_testutil::e2e::assertions::message_contains_text;
use gsm_translator::{TelegramTranslator, Translator};
use reqwest::header::RETRY_AFTER;
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};
use std::error::Error;
use std::future::Future;
use tokio::time::{Duration, sleep};

const WEATHER_CARD_PATH: &str = "../cards/samples/weather.json";

#[test]
#[ignore]
fn telegram_weather_card_e2e() {
    dotenvy::dotenv().ok();

    let token = match std::env::var("TELEGRAM_BOT_TOKEN") {
        Ok(value) if !value.trim().is_empty() => value,
        _ => {
            eprintln!("skipping telegram e2e: TELEGRAM_BOT_TOKEN missing");
            return;
        }
    };

    let chat_spec = match std::env::var("TELEGRAM_CHAT_ID") {
        Ok(value) if !value.trim().is_empty() => ChatSpecifier::Id(value),
        _ => match std::env::var("TELEGRAM_CHAT_HANDLE") {
            Ok(handle) if !handle.trim().is_empty() => ChatSpecifier::Handle(handle),
            _ => {
                eprintln!("skipping telegram e2e: set TELEGRAM_CHAT_ID or TELEGRAM_CHAT_HANDLE");
                return;
            }
        },
    };

    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");

    if let Err(err) = runtime.block_on(run_telegram_e2e(token, chat_spec)) {
        if err.downcast_ref::<NetworkUnavailable>().is_some() {
            eprintln!("skipping telegram e2e: network unavailable");
            return;
        }
        panic!("telegram e2e test failed: {err:?}");
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

async fn run_telegram_e2e(token: String, chat: ChatSpecifier) -> Result<()> {
    let client = Client::new();
    let base = format!("https://api.telegram.org/bot{token}");

    let chat_id = match chat {
        ChatSpecifier::Id(id) => id,
        ChatSpecifier::Handle(handle) => match resolve_chat_handle(&client, &base, &handle).await {
            Ok(id) => id,
            Err(err) => {
                eprintln!("skipping telegram e2e: failed to resolve {handle}: {err}");
                return Ok(());
            }
        },
    };

    let card_value = gsm_testutil::load_card!(WEATHER_CARD_PATH);
    let mut card: MessageCard =
        serde_json::from_value(card_value).context("invalid card fixture")?;
    card.body
        .retain(|block| !matches!(block, CardBlock::Image { .. }));

    let out = OutMessage {
        ctx: make_tenant_ctx("acme".into(), Some("support".into()), None),
        tenant: "acme".into(),
        platform: Platform::Telegram,
        chat_id: chat_id.clone(),
        thread_id: None,
        kind: OutKind::Card,
        text: None,
        message_card: Some(card),
        adaptive_card: None,
        meta: Default::default(),
    };

    let translator = TelegramTranslator::new();
    let payloads = translator
        .to_platform(&out)
        .context("translator failed to produce telegram payload")?;

    let mut sent_messages: Vec<Value> = Vec::new();

    for payload in payloads {
        let method = payload
            .get("method")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("translator payload missing method"))?
            .to_string();

        let mut body = payload
            .as_object()
            .cloned()
            .ok_or_else(|| anyhow!("translator payload not an object"))?;
        body.remove("method");
        body.insert("chat_id".into(), Value::String(chat_id.clone()));

        let request_url = format!("{base}/{method}");
        let response = request_with_retry(|| {
            let client = client.clone();
            let request_url = request_url.clone();
            let body = body.clone();
            async move { client.post(&request_url).json(&body).send().await }
        })
        .await
        .map_err(handle_reqwest_error)?;

        let status = response.status();
        let body: TelegramResponse = response
            .json()
            .await
            .context("failed to decode telegram response")?;

        if !body.ok {
            return Err(anyhow!(
                "telegram method {method} failed: status {status}, body {:?}",
                body.description
            ));
        }

        let message = body
            .result
            .context("telegram response missing result message")?;

        sent_messages.push(message);
    }

    let first = sent_messages
        .first()
        .cloned()
        .ok_or_else(|| anyhow!("telegram translator produced no messages"))?;

    ensure!(
        sent_messages
            .iter()
            .any(|msg| message_contains_text(msg, "Daily Weather")),
        "telegram message does not include expected title"
    );

    ensure!(
        sent_messages
            .iter()
            .any(|msg| message_contains_text(msg, "Detailed Forecast")),
        "telegram message does not include expected action label"
    );

    verify_history(&client, &base, &chat_id, &first)
        .await
        .context("failed to verify message in history")?;

    for message in &sent_messages {
        if let Some(message_id) = message.get("message_id").and_then(Value::as_i64)
            && let Err(err) = delete_telegram_message(&client, &base, &chat_id, message_id).await
        {
            eprintln!("failed to delete telegram message {message_id}: {err:#}");
        }
    }

    Ok(())
}

async fn delete_telegram_message(
    client: &Client,
    base: &str,
    chat_id: &str,
    message_id: i64,
) -> Result<()> {
    let delete_url = format!("{base}/deleteMessage");
    let response = request_with_retry(|| {
        let client = client.clone();
        let delete_url = delete_url.clone();
        let chat_id = chat_id.to_string();
        async move {
            client
                .post(&delete_url)
                .json(&json!({
                    "chat_id": chat_id,
                    "message_id": message_id,
                }))
                .send()
                .await
        }
    })
    .await
    .map_err(handle_reqwest_error)?;

    let body: TelegramResponse = response
        .json()
        .await
        .context("failed to decode deleteMessage response")?;

    if !body.ok {
        return Err(anyhow!(
            "deleteMessage returned error: {:?}",
            body.description
        ));
    }

    Ok(())
}

async fn verify_history(client: &Client, base: &str, chat_id: &str, message: &Value) -> Result<()> {
    let text = message
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let get_chat_url = format!("{base}/getChat");
    let response = request_with_retry(|| {
        let client = client.clone();
        let get_chat_url = get_chat_url.clone();
        let chat_id = chat_id.to_string();
        async move {
            client
                .post(&get_chat_url)
                .json(&json!({ "chat_id": chat_id }))
                .send()
                .await
        }
    })
    .await
    .map_err(handle_reqwest_error)?;

    let chat: TelegramChatResponse = response
        .json()
        .await
        .context("failed to decode getChat response")?;

    if !chat.ok {
        return Err(anyhow!("getChat error: {:?}", chat.description));
    }

    // Telegram API does not expose a direct chat history endpoint. The sendMessage
    // response already contains the canonical message payload; confirm the chat we
    // queried matches the target chat id and that the message text is non-empty.
    let resolved_id = chat
        .result
        .context("getChat response missing chat data")?
        .id;

    if resolved_id.to_string() != chat_id {
        return Err(anyhow!(
            "chat id mismatch: expected {chat_id}, got {resolved_id}"
        ));
    }

    ensure!(
        !text.trim().is_empty(),
        "telegram message text unexpectedly empty"
    );

    Ok(())
}

fn handle_reqwest_error(err: reqwest::Error) -> anyhow::Error {
    if err.is_timeout() || err.is_connect() {
        NetworkUnavailable.into()
    } else {
        err.into()
    }
}

async fn resolve_chat_handle(client: &Client, base: &str, handle: &str) -> Result<String> {
    let get_chat_url = format!("{base}/getChat");
    let response = request_with_retry(|| {
        let client = client.clone();
        let get_chat_url = get_chat_url.clone();
        let handle = handle.to_string();
        async move {
            client
                .post(&get_chat_url)
                .json(&json!({ "chat_id": handle }))
                .send()
                .await
        }
    })
    .await
    .map_err(handle_reqwest_error)?;

    let body: TelegramChatResponse = response
        .json()
        .await
        .context("failed to decode getChat response")?;

    if !body.ok {
        return Err(anyhow!(
            "getChat failed to resolve {handle}: {:?}",
            body.description
        ));
    }

    let chat = body.result.context("getChat response missing chat data")?;

    Ok(chat.id.to_string())
}

enum ChatSpecifier {
    Id(String),
    Handle(String),
}

#[derive(Debug, Deserialize)]
struct TelegramResponse {
    ok: bool,
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TelegramChatResponse {
    ok: bool,
    #[serde(default)]
    result: Option<TelegramChat>,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TelegramChat {
    id: i64,
    #[allow(dead_code)]
    username: Option<String>,
    #[allow(dead_code)]
    title: Option<String>,
}
