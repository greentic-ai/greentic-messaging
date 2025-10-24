//! Microsoft Teams egress adapter. Listens on NATS for `OutMessage`s, renders
//! Adaptive Cards, and posts them via the Graph API.
//!
//! ```
//! // Run with MS_GRAPH_* credentials set; messages published to
//! // `greentic.msg.out.{tenant}.teams.*` are delivered via chat.postMessage
//! // equivalents.
//! ```

use anyhow::{anyhow, Result};
use async_nats::jetstream::AckKind;
use futures::StreamExt;
use gsm_backpressure::BackpressureLimiter;
use gsm_core::{OutKind, OutMessage, Platform, TenantCtx};
use gsm_dlq::{DlqError, DlqPublisher};
use gsm_egress_common::{
    egress::bootstrap,
    telemetry::{context_from_out, record_egress_success, start_acquire_span, start_send_span},
};
use gsm_telemetry::{init_telemetry, TelemetryConfig};
use gsm_translator::teams::to_teams_adaptive;
use serde_json::json;
use std::time::Instant;
use tokio::time::{sleep, Duration};
use tracing::{event, Instrument, Level};

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
    let telemetry = TelemetryConfig::from_env("gsm-egress-teams", env!("CARGO_PKG_VERSION"));
    init_telemetry(telemetry)?;

    let nats_url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
    let tenant = std::env::var("TENANT").unwrap_or_else(|_| "acme".into());
    let graph_tenant = std::env::var("MS_GRAPH_TENANT_ID")?;
    let client_id = std::env::var("MS_GRAPH_CLIENT_ID")?;
    let client_secret = std::env::var("MS_GRAPH_CLIENT_SECRET")?;

    let queue = bootstrap(&nats_url, &tenant, Platform::Teams.as_str()).await?;
    tracing::info!(
        stream = %queue.stream,
        consumer = %queue.consumer,
        "egress-teams consuming from JetStream"
    );

    let dlq = DlqPublisher::new("egress", queue.client()).await?;

    let mut messages = queue.messages;
    let limiter = queue.limiter;

    while let Some(next) = messages.next().await {
        let msg = match next {
            Ok(msg) => msg,
            Err(err) => {
                tracing::error!("jetstream message error: {err}");
                continue;
            }
        };

        let out: OutMessage = match serde_json::from_slice(msg.payload.as_ref()) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("bad out msg: {e}");
                let _ = msg.ack().await;
                continue;
            }
        };
        if out.platform != Platform::Teams {
            if let Err(err) = msg.ack().await {
                tracing::error!("ack failed: {err}");
            }
            continue;
        }

        let ctx = context_from_out(&out);
        let msg_id = ctx
            .labels
            .msg_id
            .clone()
            .unwrap_or_else(|| out.message_id());
        let acquire_span = start_acquire_span(&ctx);
        let permit = match limiter.acquire(&out.tenant).instrument(acquire_span).await {
            Ok(p) => p,
            Err(err) => {
                tracing::error!(
                    error = %err,
                    tenant = %ctx.labels.tenant,
                    platform = "teams",
                    "failed to acquire backpressure permit"
                );
                let _ = msg.ack_with(AckKind::Nak(None)).await;
                continue;
            }
        };
        event!(
            Level::INFO,
            tenant = %ctx.labels.tenant,
            platform = "teams",
            msg_id = %msg_id,
            acquired = true,
            "backpressure permit acquired"
        );

        let send_start = Instant::now();
        let send_span = start_send_span(&ctx);
        let result = deliver(&out.ctx, &graph_tenant, &client_id, &client_secret, &out)
            .instrument(send_span)
            .await;
        drop(permit);
        let elapsed_ms = send_start.elapsed().as_secs_f64() * 1000.0;

        match result {
            Ok(()) => {
                if let Err(err) = msg.ack().await {
                    tracing::error!("ack failed: {err}");
                }
                record_egress_success(&ctx, elapsed_ms);
            }
            Err(e) => {
                tracing::error!("deliver failed: {e}");
                if let Err(err) = dlq
                    .publish(
                        &out.tenant,
                        out.platform.as_str(),
                        &msg_id,
                        3,
                        DlqError {
                            code: "E_SEND".into(),
                            message: e.to_string(),
                            stage: None,
                        },
                        &out,
                    )
                    .await
                {
                    tracing::error!("failed to publish dlq entry: {err}");
                }
                if let Err(err) = msg.ack_with(AckKind::Nak(None)).await {
                    tracing::error!("nak failed: {err}");
                }
            }
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

async fn deliver(
    ctx: &TenantCtx,
    tenant: &str,
    cid: &str,
    secret: &str,
    out: &OutMessage,
) -> Result<()> {
    let tkn = token(tenant, cid, secret).await?;
    let client = reqwest::Client::new();
    let chat_id = &out.chat_id;

    match out.kind {
        OutKind::Text => {
            let body = {
                let translate_span = tracing::info_span!(
                    "translate.run",
                    env = %ctx.env.as_str(),
                    tenant = %out.tenant,
                    platform = %out.platform.as_str(),
                    chat_id = %out.chat_id,
                    msg_id = %out.message_id()
                );
                let _guard = translate_span.enter();
                json!({
                  "body": { "contentType":"text", "content": out.text.clone().unwrap_or_default() }
                })
            };
            let url = messages_url(chat_id);
            send(&client, &tkn, &url, &body).await?;
        }
        OutKind::Card => {
            let card = out
                .message_card
                .as_ref()
                .ok_or_else(|| anyhow!("missing card"))?;
            let card_span = tracing::info_span!(
                "translate.run",
                env = %ctx.env.as_str(),
                tenant = %out.tenant,
                platform = %out.platform.as_str(),
                chat_id = %out.chat_id,
                msg_id = %out.message_id()
            );
            let _guard = card_span.enter();
            let adaptive = to_teams_adaptive(card, out)?;
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
