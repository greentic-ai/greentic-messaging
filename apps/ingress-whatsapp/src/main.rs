//! WhatsApp ingress adapter: verifies Meta webhook signatures, normalizes
//! messages into `MessageEnvelope`s, and publishes them to NATS.
//!
//! ```text
//! Meta calls `/whatsapp/webhook`; verified messages are emitted on
//! `greentic.msg.in.{tenant}.whatsapp.{from}`.
//! ```

use anyhow::Result;
use async_nats::Client as Nats;
use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::get,
    Extension, Router,
};
use gsm_core::{in_subject, MessageEnvelope, Platform};
use gsm_dlq::{DlqError, DlqPublisher};
use gsm_idempotency::{IdKey as IdemKey, IdempotencyGuard};
use gsm_ingress_common::init_guard;
use hmac::{Hmac, Mac};
use security::middleware::{handle_action, ActionContext, SharedActionContext};
use serde::Deserialize;
use serde_json::Value;
use sha2::Sha256;
use time::OffsetDateTime;
use tracing_subscriber::EnvFilter;

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone)]
struct AppState {
    nats: Nats,
    tenant: String,
    verify_token: String,
    app_secret: String,
    idem_guard: IdempotencyGuard,
    dlq: DlqPublisher,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let nats_url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
    let tenant = std::env::var("TENANT").unwrap_or_else(|_| "acme".into());
    let verify_token = std::env::var("WA_VERIFY_TOKEN").expect("WA_VERIFY_TOKEN required");
    let app_secret = std::env::var("WA_APP_SECRET").expect("WA_APP_SECRET required");
    let nats = async_nats::connect(nats_url).await?;
    let idem_guard = init_guard(&nats).await?;
    let dlq = DlqPublisher::new("ingress", nats.clone()).await?;
    let state = AppState {
        nats,
        tenant,
        verify_token,
        app_secret,
        idem_guard,
        dlq,
    };

    let mut app = Router::new().route("/whatsapp/webhook", get(verify).post(receive));

    match ActionContext::from_env(&state.nats).await {
        Ok(ctx) => {
            let shared: SharedActionContext = std::sync::Arc::new(ctx);
            app = app
                .route("/a", get(handle_action).layer(Extension(shared.clone())))
                .route("/a/whatsapp", get(handle_action).layer(Extension(shared)));
        }
        Err(err) => {
            tracing::warn!(error = %err, "action links disabled for ingress-whatsapp");
        }
    }

    let app = app.with_state(state);

    let addr: std::net::SocketAddr = std::env::var("BIND")
        .unwrap_or_else(|_| "0.0.0.0:8087".into())
        .parse()
        .unwrap();
    tracing::info!("ingress-whatsapp listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app.into_make_service()).await?;
    Ok(())
}

#[derive(Deserialize)]
struct VerifyQs {
    #[serde(rename = "hub.mode")]
    mode: Option<String>,
    #[serde(rename = "hub.challenge")]
    challenge: Option<String>,
    #[serde(rename = "hub.verify_token")]
    token: Option<String>,
}

async fn verify(State(state): State<AppState>, Query(q): Query<VerifyQs>) -> impl IntoResponse {
    if q.mode.as_deref() == Some("subscribe")
        && q.token.as_deref() == Some(state.verify_token.as_str())
    {
        (StatusCode::OK, q.challenge.unwrap_or_default())
    } else {
        (StatusCode::FORBIDDEN, "forbidden".to_string())
    }
}

async fn receive(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    if !verify_fb_sig(&state.app_secret, &headers, &body) {
        tracing::warn!("invalid whatsapp signature");
        return StatusCode::UNAUTHORIZED;
    }

    let payload: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("failed to decode payload: {e}");
            return StatusCode::BAD_REQUEST;
        }
    };

    let envelopes = extract_envelopes(&state.tenant, &payload);
    for env in envelopes {
        let span = tracing::info_span!(
            "ingress.handle",
            tenant = %env.tenant,
            platform = %env.platform.as_str(),
            chat_id = %env.chat_id,
            msg_id = %env.msg_id
        );
        let _guard = span.enter();
        let subject = in_subject(&state.tenant, env.platform.as_str(), &env.chat_id);
        let key = IdemKey {
            tenant: env.tenant.clone(),
            platform: env.platform.as_str().to_string(),
            msg_id: env.msg_id.clone(),
        };
        match state.idem_guard.should_process(&key).await {
            Ok(true) => {}
            Ok(false) => {
                tracing::info!(
                    tenant = %key.tenant,
                    platform = %key.platform,
                    msg_id = %key.msg_id,
                    "duplicate whatsapp event dropped"
                );
                continue;
            }
            Err(err) => {
                tracing::error!(
                    error = %err,
                    tenant = %key.tenant,
                    platform = %key.platform,
                    msg_id = %key.msg_id,
                    "idempotency check failed; continuing"
                );
            }
        }
        if let Err(e) = state
            .nats
            .publish(subject.clone(), serde_json::to_vec(&env).unwrap().into())
            .await
        {
            tracing::error!("publish failed on {subject}: {e}");
            if let Err(dlq_err) = state
                .dlq
                .publish(
                    &state.tenant,
                    env.platform.as_str(),
                    &env.msg_id,
                    1,
                    DlqError {
                        code: "E_PUBLISH".into(),
                        message: e.to_string(),
                        stage: None,
                    },
                    &env,
                )
                .await
            {
                tracing::error!("failed to publish dlq entry: {dlq_err}");
            }
            return StatusCode::INTERNAL_SERVER_ERROR;
        } else {
            metrics::counter!(
                "messages_ingressed",
                1,
                "tenant" => env.tenant.clone(),
                "platform" => env.platform.as_str().to_string()
            );
        }
    }

    StatusCode::OK
}

fn verify_fb_sig(app_secret: &str, headers: &HeaderMap, body: &[u8]) -> bool {
    let sig = headers
        .get("X-Hub-Signature-256")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !sig.starts_with("sha256=") {
        return false;
    }
    let provided = &sig[7..];
    let mut mac = match HmacSha256::new_from_slice(app_secret.as_bytes()) {
        Ok(mac) => mac,
        Err(_) => return false,
    };
    mac.update(body);
    let digest = mac.finalize().into_bytes();
    constant_time_eq(provided.as_bytes(), hex_encode(&digest).as_bytes())
}

fn extract_envelopes(tenant: &str, value: &Value) -> Vec<MessageEnvelope> {
    let mut out = Vec::new();
    let entries = match value.get("entry").and_then(|v| v.as_array()) {
        Some(entries) => entries,
        None => return out,
    };

    for entry in entries {
        let changes = match entry.get("changes").and_then(|v| v.as_array()) {
            Some(changes) => changes,
            None => continue,
        };
        for change in changes {
            let Some(value) = change.get("value") else {
                continue;
            };
            let Some(messages) = value.get("messages").and_then(|v| v.as_array()) else {
                continue;
            };
            for message in messages {
                if let Some(env) = envelope_from_message(tenant, message) {
                    out.push(env);
                }
            }
        }
    }
    out
}

fn envelope_from_message(tenant: &str, message: &Value) -> Option<MessageEnvelope> {
    let from = message.get("from")?.as_str()?.to_string();
    let chat_id = from.clone();
    let text = message
        .get("text")
        .and_then(|t| t.get("body"))
        .and_then(|b| b.as_str())
        .map(|s| s.to_string());
    let ts_str = message
        .get("timestamp")
        .and_then(|v| v.as_str())
        .unwrap_or("0");
    let ts = ts_str.parse::<i64>().ok();
    let timestamp = ts
        .and_then(|secs| OffsetDateTime::from_unix_timestamp(secs).ok())
        .unwrap_or_else(OffsetDateTime::now_utc)
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".into());

    Some(MessageEnvelope {
        tenant: tenant.to_string(),
        platform: Platform::WhatsApp,
        chat_id,
        user_id: from,
        thread_id: None,
        msg_id: format!("wa:{ts_str}"),
        text,
        timestamp,
        context: Default::default(),
    })
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn verify_fb_sig_accepts_valid_signature() {
        let secret = "secret";
        let body = b"{\"entry\":[]}";
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        let digest = mac.finalize().into_bytes();
        let sig = format!("sha256={}", hex_encode(&digest));

        let mut headers = HeaderMap::new();
        headers.insert("X-Hub-Signature-256", HeaderValue::from_str(&sig).unwrap());
        assert!(verify_fb_sig(secret, &headers, body));
    }

    #[test]
    fn verify_fb_sig_rejects_bad_signature() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "X-Hub-Signature-256",
            HeaderValue::from_static("sha256=deadbeef"),
        );
        assert!(!verify_fb_sig("secret", &headers, b"{}"));
    }

    #[test]
    fn extract_envelopes_returns_message() {
        let sample = serde_json::json!({
            "entry": [
                {"changes": [
                    {"value": {
                        "contacts": [],
                        "messages": [
                            {
                                "from": "12345",
                                "timestamp": "1700000000",
                                "text": {"body": "Hi"}
                            }
                        ]
                    }}
                ]}
            ]
        });
        let envs = extract_envelopes("tenant", &sample);
        assert_eq!(envs.len(), 1);
        let env = &envs[0];
        assert_eq!(env.chat_id, "12345");
        assert_eq!(env.text.as_deref(), Some("Hi"));
        assert_eq!(env.platform, Platform::WhatsApp);
    }
}
