#![cfg(feature = "e2e")]

use anyhow::{Context, Result, anyhow, ensure};
use gsm_core::{MessageCard, OutKind, OutMessage, Platform, make_tenant_ctx};
use gsm_testutil::load_card;
use gsm_translator::teams::to_teams_adaptive;
use reqwest::header::RETRY_AFTER;
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};
use std::error::Error;
use std::future::Future;
use tokio::time::{Duration, sleep};

const APPROVAL_CARD_PATH: &str = "../cards/samples/approval.json";

#[test]
#[ignore]
fn teams_adaptive_card_e2e() {
    dotenvy::dotenv().ok();

    let tenant_id = match std::env::var("TEAMS_TENANT_ID") {
        Ok(value) if !value.trim().is_empty() => value,
        _ => {
            eprintln!("skipping teams e2e: TEAMS_TENANT_ID missing");
            return;
        }
    };

    let client_id = match std::env::var("TEAMS_CLIENT_ID") {
        Ok(value) if !value.trim().is_empty() => value,
        _ => {
            eprintln!("skipping teams e2e: TEAMS_CLIENT_ID missing");
            return;
        }
    };

    let client_secret = match std::env::var("TEAMS_CLIENT_SECRET") {
        Ok(value) if !value.trim().is_empty() => value,
        _ => {
            eprintln!("skipping teams e2e: TEAMS_CLIENT_SECRET missing");
            return;
        }
    };

    let chat_id = match std::env::var("TEAMS_CHAT_ID") {
        Ok(value) if !value.trim().is_empty() => value,
        _ => {
            eprintln!("skipping teams e2e: TEAMS_CHAT_ID missing");
            return;
        }
    };

    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");

    if let Err(err) = runtime.block_on(run_teams_e2e(tenant_id, client_id, client_secret, chat_id))
    {
        if err.downcast_ref::<NetworkUnavailable>().is_some() {
            eprintln!("skipping teams e2e: network unavailable");
            return;
        }
        panic!("teams e2e test failed: {err:?}");
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

async fn run_teams_e2e(
    tenant_id: String,
    client_id: String,
    client_secret: String,
    chat_id: String,
) -> Result<()> {
    let client = Client::new();
    let token = acquire_token(&client, &tenant_id, &client_id, &client_secret).await?;

    let card_value = load_card!(APPROVAL_CARD_PATH);
    let mut card: MessageCard =
        serde_json::from_value(card_value).context("invalid card fixture")?;

    // Ensure actions are graph-friendly (JWT signing not supported here).
    for action in &mut card.actions {
        if let gsm_core::CardAction::OpenUrl { jwt, .. } = action {
            *jwt = false;
        }
    }

    let mut adaptive_card = to_teams_adaptive(
        &card,
        &OutMessage {
            ctx: make_tenant_ctx("acme".into(), Some("finance".into()), None),
            tenant: "acme".into(),
            platform: Platform::Teams,
            chat_id: chat_id.clone(),
            thread_id: None,
            kind: OutKind::Card,
            text: None,
            message_card: None,
            adaptive_card: None,
            meta: Default::default(),
        },
    )
    .context("failed to generate adaptive card")?;

    if let Some(obj) = adaptive_card.as_object_mut() {
        obj.insert("version".into(), Value::String("1.3".into()));
    }

    let attachment_id = "1";
    let message_payload = json!({
        "subject": null,
        "body": {
            "contentType": "html",
            "content": format!("<attachment id=\"{}\"></attachment>", attachment_id)
        },
        "attachments": [
            {
                "id": attachment_id,
                "contentType": "application/vnd.microsoft.card.adaptive",
                "content": adaptive_card
            }
        ]
    });

    let post_url = format!("https://graph.microsoft.com/v1.0/chats/{chat_id}/messages");
    let post_response = request_with_retry(|| {
        let client = client.clone();
        let token = token.clone();
        let message_payload = message_payload.clone();
        let post_url = post_url.clone();
        async move {
            client
                .post(&post_url)
                .bearer_auth(&token)
                .json(&message_payload)
                .send()
                .await
        }
    })
    .await
    .map_err(handle_reqwest_error)?;

    let post_status = post_response.status();
    let post_body_text = post_response
        .text()
        .await
        .context("failed to read teams post response")?;

    if !post_status.is_success() {
        return Err(anyhow!(
            "teams message failed: status {post_status}, body {post_body_text}"
        ));
    }

    let post_body: Value =
        serde_json::from_str(&post_body_text).context("failed to parse teams post response")?;
    let message_id = post_body
        .get("id")
        .and_then(Value::as_str)
        .context("teams response missing message id")?
        .to_string();

    // Allow brief propagation before fetching the message.
    sleep(Duration::from_secs(2)).await;

    let fetch_url =
        format!("https://graph.microsoft.com/v1.0/chats/{chat_id}/messages/{message_id}");
    let fetched = request_with_retry(|| {
        let client = client.clone();
        let token = token.clone();
        let fetch_url = fetch_url.clone();
        async move { client.get(&fetch_url).bearer_auth(&token).send().await }
    })
    .await
    .map_err(handle_reqwest_error)?;

    let fetch_status = fetched.status();
    let fetch_body: Value = fetched
        .json()
        .await
        .context("failed to decode teams message query")?;

    if !fetch_status.is_success() {
        return Err(anyhow!(
            "teams message query failed: status {fetch_status}, body {fetch_body}"
        ));
    }

    let attachments = fetch_body
        .get("attachments")
        .and_then(Value::as_array)
        .context("teams message missing attachments")?;

    let first = attachments
        .first()
        .context("teams message attachments empty")?;

    ensure!(
        first
            .get("contentType")
            .and_then(Value::as_str)
            .map(|value| value.eq_ignore_ascii_case("application/vnd.microsoft.card.adaptive"))
            .unwrap_or(false),
        "teams attachment contentType mismatch"
    );

    let card_content = first
        .get("content")
        .and_then(|content| match content {
            Value::String(raw) => serde_json::from_str::<Value>(raw).ok(),
            Value::Object(_) | Value::Array(_) => Some(content.clone()),
            _ => None,
        })
        .context("teams attachment missing card content")?;

    ensure!(
        card_content.get("type").and_then(Value::as_str) == Some("AdaptiveCard"),
        "teams adaptive card type mismatch"
    );

    if let Err(err) = delete_teams_message(&client, &token, &chat_id, &message_id).await {
        eprintln!("failed to delete teams message {message_id}: {err:#}");
    }

    Ok(())
}

async fn acquire_token(
    client: &Client,
    tenant_id: &str,
    client_id: &str,
    client_secret: &str,
) -> Result<String> {
    let token_url = format!("https://login.microsoftonline.com/{tenant_id}/oauth2/v2.0/token");
    let client_id_owned = client_id.to_string();
    let client_secret_owned = client_secret.to_string();
    let response = request_with_retry(|| {
        let client = client.clone();
        let token_url = token_url.clone();
        let client_id = client_id_owned.clone();
        let client_secret = client_secret_owned.clone();
        async move {
            client
                .post(&token_url)
                .form(&[
                    ("grant_type", "client_credentials"),
                    ("client_id", client_id.as_str()),
                    ("client_secret", client_secret.as_str()),
                    ("scope", "https://graph.microsoft.com/.default"),
                ])
                .send()
                .await
        }
    })
    .await
    .map_err(handle_reqwest_error)?;

    let status = response.status();
    let body: TokenResponse = response
        .json()
        .await
        .context("failed to decode token response")?;

    if !status.is_success() {
        return Err(anyhow!("token request failed: status {status}"));
    }

    body.access_token
        .context("token response missing access_token")
}

async fn delete_teams_message(
    client: &Client,
    token: &str,
    chat_id: &str,
    message_id: &str,
) -> Result<()> {
    let delete_url =
        format!("https://graph.microsoft.com/v1.0/chats/{chat_id}/messages/{message_id}");
    let response = request_with_retry(|| {
        let client = client.clone();
        let token = token.to_string();
        let delete_url = delete_url.clone();
        async move { client.delete(&delete_url).bearer_auth(&token).send().await }
    })
    .await
    .map_err(handle_reqwest_error)?;

    let status = response.status();
    if status.is_success() {
        Ok(())
    } else {
        let body = response.text().await.unwrap_or_default();
        Err(anyhow!(
            "failed to delete teams message: status {status}, body {body}"
        ))
    }
}

fn handle_reqwest_error(err: reqwest::Error) -> anyhow::Error {
    if err.is_connect() || err.is_timeout() {
        NetworkUnavailable.into()
    } else {
        err.into()
    }
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    #[serde(default)]
    access_token: Option<String>,
}
