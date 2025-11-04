#![cfg(feature = "e2e")]

use anyhow::{Context, Result, anyhow, ensure};
use gsm_core::{CardAction, MessageCard, OutKind, OutMessage, Platform, make_tenant_ctx};
use gsm_testutil::{load_card, visual};
use gsm_translator::teams::to_teams_adaptive;
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;
use std::error::Error;

const APPROVAL_CARD_PATH: &str = "../cards/samples/approval.json";

#[test]
#[ignore]
fn webex_adaptive_card_e2e() {
    dotenvy::dotenv().ok();

    let token = match std::env::var("WEBEX_BOT_TOKEN") {
        Ok(value) if !value.trim().is_empty() => value,
        _ => {
            eprintln!("skipping webex e2e: WEBEX_BOT_TOKEN missing");
            return;
        }
    };

    let room_id = match std::env::var("WEBEX_ROOM_ID") {
        Ok(value) if !value.trim().is_empty() => value,
        _ => {
            eprintln!("skipping webex e2e: WEBEX_ROOM_ID missing");
            return;
        }
    };

    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");

    if let Err(err) = runtime.block_on(run_webex_e2e(token, room_id)) {
        if err.downcast_ref::<NetworkUnavailable>().is_some() {
            eprintln!("skipping webex e2e: network unavailable");
            return;
        }
        panic!("webex e2e test failed: {err:?}");
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

async fn run_webex_e2e(token: String, room_id: String) -> Result<()> {
    let client = Client::new();

    let card_value = load_card!(APPROVAL_CARD_PATH);
    let mut card: MessageCard =
        serde_json::from_value(card_value).context("invalid card fixture")?;
    for action in &mut card.actions {
        if let CardAction::OpenUrl { jwt, .. } = action {
            *jwt = false;
        }
    }

    let out = OutMessage {
        ctx: make_tenant_ctx("acme".into(), Some("finance".into()), None),
        tenant: "acme".into(),
        platform: Platform::Teams,
        chat_id: room_id.clone(),
        thread_id: None,
        kind: OutKind::Card,
        text: None,
        message_card: Some(card),
        meta: Default::default(),
    };

    let mut adaptive_card = to_teams_adaptive(out.message_card.as_ref().unwrap(), &out)
        .context("failed to translate card to adaptive")?;
    if let Some(obj) = adaptive_card.as_object_mut() {
        obj.insert("version".into(), Value::String("1.3".into()));
    }

    let message_payload = serde_json::json!({
        "roomId": room_id,
        "markdown": "Greentic approval card",
        "attachments": [
            {
                "contentType": "application/vnd.microsoft.card.adaptive",
                "content": adaptive_card
            }
        ]
    });

    let post_response = client
        .post("https://webexapis.com/v1/messages")
        .bearer_auth(&token)
        .json(&message_payload)
        .send()
        .await
        .map_err(handle_reqwest_error)?;

    let post_status = post_response.status();
    let post_body_text = post_response
        .text()
        .await
        .context("failed to read webex create response")?;

    let post_body: WebexMessage = serde_json::from_str(&post_body_text).unwrap_or_default();

    if post_status.is_client_error() || post_status.is_server_error() {
        return Err(anyhow!(
            "webex message creation failed: status {post_status}, body {post_body_text}"
        ));
    }

    let message_id = post_body.id.context("webex response missing message id")?;

    let fetched = client
        .get(&format!("https://webexapis.com/v1/messages/{message_id}"))
        .bearer_auth(&token)
        .send()
        .await
        .map_err(handle_reqwest_error)?;

    let fetched_body: WebexMessage = fetched
        .json()
        .await
        .context("failed to decode webex message detail")?;

    let attachments = fetched_body
        .attachments
        .as_ref()
        .ok_or_else(|| anyhow!("webex message missing attachments"))?;

    let first = attachments
        .first()
        .ok_or_else(|| anyhow!("webex message attachments empty"))?;

    ensure!(
        first
            .content_type
            .as_deref()
            .map(|value| value.eq_ignore_ascii_case("application/vnd.microsoft.card.adaptive"))
            .unwrap_or(false),
        "webex attachment content type mismatch"
    );

    ensure!(
        first
            .content
            .as_ref()
            .and_then(|c| c.get("type"))
            .and_then(Value::as_str)
            == Some("AdaptiveCard"),
        "webex attachment content.type was not AdaptiveCard"
    );

    if let Some(card_value) = first.content.as_ref() {
        if let Ok(rendered_path) = visual::try_render_adaptive_card_to_png(card_value) {
            println!(
                "rendered adaptive card snapshot at {}",
                rendered_path.display()
            );
        }
    }

    Ok(())
}

fn handle_reqwest_error(err: reqwest::Error) -> anyhow::Error {
    if err.is_timeout() || err.is_connect() {
        NetworkUnavailable.into()
    } else {
        err.into()
    }
}

#[derive(Debug, Deserialize)]
struct WebexMessage {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    attachments: Option<Vec<WebexAttachment>>,
}

impl Default for WebexMessage {
    fn default() -> Self {
        Self {
            id: None,
            attachments: None,
        }
    }
}

#[derive(Debug, Deserialize)]
struct WebexAttachment {
    #[serde(rename = "contentType")]
    #[serde(default)]
    content_type: Option<String>,
    #[serde(default)]
    content: Option<Value>,
}
