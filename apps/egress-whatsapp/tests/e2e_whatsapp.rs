#![cfg(feature = "e2e")]

use anyhow::{Context, Result, anyhow};
use gsm_core::{CardAction, CardBlock, MessageCard};
use gsm_testutil::load_card;
use reqwest::header::RETRY_AFTER;
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::error::Error;
use std::future::Future;
use tokio::time::{Duration, sleep};

const APPROVAL_CARD_PATH: &str = "../cards/samples/approval.json";

#[test]
#[ignore]
fn whatsapp_interactive_card_e2e() {
    dotenvy::dotenv().ok();

    let token = match std::env::var("WHATSAPP_TOKEN") {
        Ok(value) if !value.trim().is_empty() => value,
        _ => {
            eprintln!("skipping whatsapp e2e: WHATSAPP_TOKEN missing");
            return;
        }
    };

    let phone_id = match std::env::var("WHATSAPP_PHONE_ID") {
        Ok(value) if !value.trim().is_empty() => value,
        _ => {
            eprintln!("skipping whatsapp e2e: WHATSAPP_PHONE_ID missing");
            return;
        }
    };

    let recipient = match std::env::var("WHATSAPP_RECIPIENT") {
        Ok(value) if !value.trim().is_empty() => value,
        _ => {
            eprintln!("skipping whatsapp e2e: WHATSAPP_RECIPIENT missing");
            return;
        }
    };

    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    if let Err(err) = runtime.block_on(run_whatsapp_e2e(token, phone_id, recipient)) {
        if err.downcast_ref::<NetworkUnavailable>().is_some() {
            eprintln!("skipping whatsapp e2e: network unavailable");
            return;
        }
        panic!("whatsapp e2e test failed: {err:?}");
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

async fn run_whatsapp_e2e(token: String, phone_id: String, recipient: String) -> Result<()> {
    let client = Client::new();
    let card_value = load_card!(APPROVAL_CARD_PATH);
    let card: MessageCard = serde_json::from_value(card_value).context("invalid card fixture")?;

    let body_text = build_body_text(&card);
    let buttons = build_buttons(&card);

    let mut request_body = HashMap::new();
    request_body.insert("messaging_product", Value::String("whatsapp".into()));
    request_body.insert("to", Value::String(recipient));
    request_body.insert("type", Value::String("interactive".into()));

    let mut interactive = Map::new();
    interactive.insert("type".into(), Value::String("button".into()));
    interactive.insert(
        "body".into(),
        Value::Object({
            let mut body = Map::new();
            body.insert("text".into(), Value::String(body_text));
            body
        }),
    );
    interactive.insert(
        "action".into(),
        Value::Object({
            let mut action = Map::new();
            action.insert("buttons".into(), Value::Array(buttons));
            action
        }),
    );

    request_body.insert("interactive", Value::Object(interactive));

    let send_url = format!("https://graph.facebook.com/v18.0/{phone_id}/messages");
    let send_response = request_with_retry(|| {
        let client = client.clone();
        let token = token.clone();
        let send_url = send_url.clone();
        let request_body = request_body.clone();
        async move {
            client
                .post(&send_url)
                .bearer_auth(&token)
                .json(&request_body)
                .send()
                .await
        }
    })
    .await
    .map_err(handle_reqwest_error)?;

    let send_status = send_response.status();
    let send_body: WhatsAppSendResponse = send_response
        .json()
        .await
        .context("failed to decode whatsapp send response")?;

    if send_status.is_client_error() || send_status.is_server_error() {
        return Err(anyhow!(
            "whatsapp send failed: status {send_status}, body {:?}",
            send_body
        ));
    }

    let message_id = send_body
        .messages
        .as_ref()
        .and_then(|m| m.first())
        .map(|m| m.id.clone())
        .context("whatsapp send response missing message id")?;

    // Query the message delivery state.
    let fetch_url =
        format!("https://graph.facebook.com/v18.0/{phone_id}/messages?message_id={message_id}");
    let fetch_response = request_with_retry(|| {
        let client = client.clone();
        let token = token.clone();
        let fetch_url = fetch_url.clone();
        async move { client.get(&fetch_url).bearer_auth(&token).send().await }
    })
    .await
    .map_err(handle_reqwest_error)?;

    let fetch_status = fetch_response.status();
    let fetch_body: WhatsAppMessagesQuery = fetch_response
        .json()
        .await
        .context("failed to decode whatsapp messages query")?;

    if fetch_status.is_client_error() || fetch_status.is_server_error() {
        return Err(anyhow!(
            "whatsapp messages query failed: status {fetch_status}, body {:?}",
            fetch_body
        ));
    }

    let delivered = fetch_body
        .data
        .as_ref()
        .and_then(|list| list.first())
        .map(|entry| entry.status.clone())
        .unwrap_or_default();

    if delivered.is_empty() {
        return Err(anyhow!("whatsapp messages query returned empty status"));
    }

    println!("whatsapp message {message_id} status: {delivered}");

    if let Err(err) = delete_whatsapp_message(&client, &token, &phone_id, &message_id).await {
        eprintln!("failed to delete whatsapp message {message_id}: {err:#}");
    }

    Ok(())
}

async fn delete_whatsapp_message(
    client: &Client,
    token: &str,
    phone_id: &str,
    message_id: &str,
) -> Result<()> {
    let delete_url = format!("https://graph.facebook.com/v18.0/{phone_id}/messages");
    let response = request_with_retry(|| {
        let client = client.clone();
        let token = token.to_string();
        let delete_url = delete_url.clone();
        let message_id = message_id.to_string();
        async move {
            client
                .delete(&delete_url)
                .bearer_auth(&token)
                .query(&[("message_id", message_id.as_str())])
                .send()
                .await
        }
    })
    .await
    .map_err(handle_reqwest_error)?;

    let status = response.status();
    if status.is_success() {
        Ok(())
    } else {
        let body = response.text().await.unwrap_or_default();
        Err(anyhow!(
            "failed to delete whatsapp message: status {status}, body {body}"
        ))
    }
}

fn build_body_text(card: &MessageCard) -> String {
    let mut lines: Vec<String> = Vec::new();
    if let Some(title) = &card.title {
        lines.push(title.clone());
    }
    for block in &card.body {
        match block {
            CardBlock::Text { text, .. } => lines.push(text.clone()),
            CardBlock::Fact { label, value } => {
                lines.push(format!("{label}: {value}"));
            }
            CardBlock::Image { .. } => {}
        }
    }
    lines.truncate(5);
    lines.join("\n")
}

fn build_buttons(card: &MessageCard) -> Vec<Value> {
    let mut buttons = Vec::new();
    for (index, action) in card.actions.iter().enumerate() {
        if index >= 3 {
            break;
        }
        if let CardAction::Postback { title, .. } = action {
            buttons.push(Value::Object(button_payload(index, title)));
        } else if let CardAction::OpenUrl { title, url, .. } = action {
            buttons.push(Value::Object(button_payload(
                index,
                &format!("{title} ({url})"),
            )));
        }
    }

    if buttons.is_empty() {
        buttons.push(Value::Object(button_payload(0, "Approve")));
    }

    buttons
}

fn button_payload(index: usize, title: &str) -> Map<String, Value> {
    let mut reply = Map::new();
    reply.insert("id".into(), Value::String(format!("btn_{index}")));
    reply.insert(
        "title".into(),
        Value::String(title.chars().take(20).collect()),
    );

    let mut button = Map::new();
    button.insert("type".into(), Value::String("reply".into()));
    button.insert("reply".into(), Value::Object(reply));
    button
}

fn handle_reqwest_error(err: reqwest::Error) -> anyhow::Error {
    if err.is_timeout() || err.is_connect() {
        NetworkUnavailable.into()
    } else {
        err.into()
    }
}

#[derive(Debug, Deserialize)]
struct WhatsAppSendResponse {
    #[serde(default)]
    messaging_product: Option<String>,
    #[serde(default)]
    messages: Option<Vec<WhatsAppMessageRef>>,
    #[serde(default)]
    contacts: Option<Vec<Value>>,
    #[serde(default)]
    error: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct WhatsAppMessageRef {
    id: String,
}

#[derive(Debug, Deserialize)]
struct WhatsAppMessagesQuery {
    #[serde(default)]
    data: Option<Vec<WhatsAppMessageStatus>>,
}

#[derive(Debug, Deserialize)]
struct WhatsAppMessageStatus {
    #[serde(default)]
    status: String,
}
