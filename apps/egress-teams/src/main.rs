//! Microsoft Teams egress adapter. Listens on NATS for `OutMessage`s, renders
//! Adaptive Cards, and posts them via the Graph API.
//!
//! ```
//! // Run with MS_GRAPH_* credentials set; messages published to
//! // `greentic.msg.out.{tenant}.teams.*` are delivered via chat.postMessage
//! // equivalents.
//! ```

use anyhow::{anyhow, Result};
use futures::StreamExt;
use gsm_core::{OutKind, OutMessage, Platform};
use gsm_translator::teams::to_teams_adaptive;
use serde_json::json;
use tokio::time::{sleep, Duration};
use tracing_subscriber::EnvFilter;

fn auth_base() -> String {
    std::env::var("MS_GRAPH_AUTH_BASE")
        .unwrap_or_else(|_| "https://login.microsoftonline.com".into())
}

fn api_base() -> String {
    std::env::var("MS_GRAPH_API_BASE").unwrap_or_else(|_| "https://graph.microsoft.com".into())
}

fn token_url(tenant: &str) -> String {
    let base = auth_base();
    let base = base.trim_end_matches('/');
    format!("{base}/{tenant}/oauth2/v2.0/token")
}

fn messages_url(chat_id: &str) -> String {
    let base = api_base();
    let base = base.trim_end_matches('/');
    format!("{base}/v1.0/chats/{chat_id}/messages")
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let nats_url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
    let tenant = std::env::var("TENANT").unwrap_or_else(|_| "acme".into());
    let graph_tenant = std::env::var("MS_GRAPH_TENANT_ID")?;
    let client_id = std::env::var("MS_GRAPH_CLIENT_ID")?;
    let client_secret = std::env::var("MS_GRAPH_CLIENT_SECRET")?;

    let nats = async_nats::connect(nats_url).await?;
    let subject = format!("greentic.msg.out.{}.teams.>", tenant);
    let mut sub = nats.subscribe(subject.clone()).await?;
    tracing::info!("egress-teams subscribed to {subject}");

    while let Some(msg) = sub.next().await {
        let out: OutMessage = match serde_json::from_slice(&msg.payload) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("bad out msg: {e}");
                continue;
            }
        };
        if out.platform != Platform::Teams {
            continue;
        }
        if let Err(e) = deliver(&graph_tenant, &client_id, &client_secret, &out).await {
            tracing::error!("deliver failed: {e}");
        }
    }

    Ok(())
}

async fn token(tenant: &str, client_id: &str, client_secret: &str) -> Result<String> {
    let url = token_url(tenant);
    let form = [
        ("client_id", client_id),
        ("client_secret", client_secret),
        ("grant_type", "client_credentials"),
        ("scope", "https://graph.microsoft.com/.default"),
    ];
    let res = reqwest::Client::new().post(url).form(&form).send().await?;
    let status = res.status();
    let bytes = res.bytes().await?;
    if !status.is_success() {
        let body = String::from_utf8_lossy(&bytes);
        return Err(anyhow!("token request failed: {} {}", status, body));
    }
    let v: serde_json::Value = serde_json::from_slice(&bytes)?;
    Ok(v["access_token"].as_str().unwrap_or_default().to_string())
}

async fn deliver(tenant: &str, cid: &str, secret: &str, out: &OutMessage) -> Result<()> {
    let tkn = token(tenant, cid, secret).await?;
    let client = reqwest::Client::new();
    let chat_id = &out.chat_id;

    match out.kind {
        OutKind::Text => {
            let body = json!({
              "body": { "contentType":"text", "content": out.text.clone().unwrap_or_default() }
            });
            let url = messages_url(chat_id);
            send(&client, &tkn, &url, &body).await?;
        }
        OutKind::Card => {
            let card = out
                .message_card
                .as_ref()
                .ok_or_else(|| anyhow!("missing card"))?;
            let adaptive = to_teams_adaptive(card)?;
            let body = json!({
              "subject": null,
              "importance": "normal",
              "body": { "contentType": "html", "content": " " },
              "attachments": [{
                "id": "1",
                "contentType": "application/vnd.microsoft.card.adaptive",
                "contentUrl": null,
                "content": adaptive,
                "name": "card.json",
                "thumbnailUrl": null
              }]
            });
            let url = messages_url(chat_id);
            send(&client, &tkn, &url, &body).await?;
        }
    }
    Ok(())
}

async fn send(
    client: &reqwest::Client,
    token: &str,
    url: &str,
    body: &serde_json::Value,
) -> Result<()> {
    for attempt in 0..=2 {
        let res = client.post(url).bearer_auth(token).json(body).send().await;
        match res {
            Ok(r) if r.status().is_success() => return Ok(()),
            Ok(r) => {
                let status = r.status();
                let txt = r.text().await.unwrap_or_default();
                tracing::warn!("graph err {}: {}", status, txt);
            }
            Err(e) => tracing::warn!("graph send error: {e}"),
        }
        sleep(Duration::from_millis(250 * (attempt + 1))).await;
    }
    Err(anyhow!("send failed after retries"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn token_url_uses_auth_base_override() {
        let _guard = env_lock().lock().unwrap();
        std::env::set_var("MS_GRAPH_AUTH_BASE", "https://example.com");
        let url = token_url("tenant");
        std::env::remove_var("MS_GRAPH_AUTH_BASE");
        assert_eq!(url, "https://example.com/tenant/oauth2/v2.0/token");
    }

    #[test]
    fn messages_url_uses_api_base() {
        let _guard = env_lock().lock().unwrap();
        std::env::set_var("MS_GRAPH_API_BASE", "https://api.example.com");
        let url = messages_url("chat-1");
        std::env::remove_var("MS_GRAPH_API_BASE");
        assert_eq!(url, "https://api.example.com/v1.0/chats/chat-1/messages");
    }

    #[test]
    fn messages_url_trims_trailing_slash() {
        let _guard = env_lock().lock().unwrap();
        std::env::set_var("MS_GRAPH_API_BASE", "https://api.example.com/");
        let url = messages_url("chat-1");
        std::env::remove_var("MS_GRAPH_API_BASE");
        assert_eq!(url, "https://api.example.com/v1.0/chats/chat-1/messages");
    }

    #[test]
    fn api_base_defaults_to_graph_endpoint() {
        let _guard = env_lock().lock().unwrap();
        std::env::remove_var("MS_GRAPH_API_BASE");
        assert_eq!(api_base(), "https://graph.microsoft.com");
    }
}
