//! Slack Events API ingress service.
//!
//! Exposes a `/slack/events` endpoint that validates the Slack signing secret,
//! acknowledges URL verification challenges, and forwards normalized messages to NATS.

use anyhow::Result;
use async_nats::Client as Nats;
use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Router,
};
use gsm_core::{in_subject, MessageEnvelope, Platform};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use time::OffsetDateTime;
use tracing_subscriber::EnvFilter;

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone)]
struct AppState {
    nats: Nats,
    tenant: String,
    signing_secret: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let nats_url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
    let tenant = std::env::var("TENANT").unwrap_or_else(|_| "acme".into());
    let signing_secret =
        std::env::var("SLACK_SIGNING_SECRET").expect("SLACK_SIGNING_SECRET required");
    let nats = async_nats::connect(nats_url).await?;
    let state = AppState {
        nats,
        tenant,
        signing_secret,
    };

    let app = Router::new()
        .route("/slack/events", post(handle))
        .with_state(state);

    let addr: std::net::SocketAddr = std::env::var("BIND")
        .unwrap_or_else(|_| "0.0.0.0:8086".into())
        .parse()
        .unwrap();
    tracing::info!("ingress-slack listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app.into_make_service()).await?;
    Ok(())
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
struct SlackEnvelope {
    #[serde(default)]
    r#type: Option<String>,
    #[serde(default)]
    token: Option<String>,
    #[serde(default)]
    challenge: Option<String>,
    #[serde(default)]
    event: Option<SlackEvent>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
struct SlackEvent {
    #[serde(default)]
    r#type: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    user: Option<String>,
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    ts: Option<String>,
    #[serde(default)]
    thread_ts: Option<String>,
}

async fn handle(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    if let Some(challenge) = extract_challenge(&body) {
        return (StatusCode::OK, challenge).into_response();
    }

    if !verify_slack_sig(&state.signing_secret, &headers, &body) {
        tracing::warn!("invalid slack signature");
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let envelope: SlackEnvelope = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!("slack payload parse error: {error}");
            return StatusCode::BAD_REQUEST.into_response();
        }
    };

    if let Some(event) = envelope.event {
        match map_slack_event(&state.tenant, event) {
            Some((subject, message)) => match serde_json::to_vec(&message) {
                Ok(bytes) => {
                    if let Err(error) = state.nats.publish(subject.clone(), bytes.into()).await {
                        tracing::error!("publish failed on {subject}: {error}");
                        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                    }
                    tracing::info!("published slack event for {}", message.chat_id);
                }
                Err(error) => {
                    tracing::error!("serialize envelope failed: {error}");
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }
            },
            None => return StatusCode::BAD_REQUEST.into_response(),
        }
    }

    StatusCode::OK.into_response()
}

fn extract_challenge(body: &[u8]) -> Option<String> {
    let mut envelope: SlackEnvelope = serde_json::from_slice(body).ok()?;
    if envelope.r#type.as_deref() == Some("url_verification") {
        return envelope.challenge.take();
    }
    None
}

/// Verifies Slack's signed request using the signing secret.
///
/// ```
/// use axum::http::HeaderMap;
/// use gsm_ingress_slack::verify_slack_sig;
///
/// let mut headers = HeaderMap::new();
/// headers.insert("X-Slack-Request-Timestamp", "1".parse().unwrap());
/// headers.insert("X-Slack-Signature", "v0=deadbeef".parse().unwrap());
/// assert!(!verify_slack_sig("secret", &headers, b"{}"));
/// ```
pub fn verify_slack_sig(secret: &str, headers: &HeaderMap, body: &[u8]) -> bool {
    let timestamp = headers
        .get("X-Slack-Request-Timestamp")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let signature = headers
        .get("X-Slack-Signature")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");

    if timestamp.is_empty() || signature.is_empty() {
        return false;
    }

    let base_string = format!("v0:{timestamp}:{}", String::from_utf8_lossy(body));
    let mut mac = match HmacSha256::new_from_slice(secret.as_bytes()) {
        Ok(mac) => mac,
        Err(_) => return false,
    };
    mac.update(base_string.as_bytes());
    let digest = mac.finalize().into_bytes();
    let calc = format!("v0={}", hex_encode(digest.as_ref()));
    subtle_constant_time_eq(&calc, signature)
}

fn map_slack_event(tenant: &str, event: SlackEvent) -> Option<(String, MessageEnvelope)> {
    let chat_id = event.channel?;
    let user_id = event.user.unwrap_or_else(|| "unknown".into());
    let timestamp = event.ts.unwrap_or_else(|| "0".into());
    let now = OffsetDateTime::now_utc();
    let envelope = MessageEnvelope {
        tenant: tenant.to_string(),
        platform: Platform::Slack,
        chat_id: chat_id.clone(),
        user_id,
        thread_id: event.thread_ts,
        msg_id: format!("slack:{timestamp}"),
        text: event.text,
        timestamp: now
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_else(|_| "1970-01-01T00:00:00Z".into()),
        context: Default::default(),
    };

    let subject = in_subject(tenant, Platform::Slack.as_str(), &chat_id);
    Some((subject, envelope))
}

fn subtle_constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.bytes().zip(b.bytes()) {
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
    use axum::http::HeaderMap;

    #[test]
    fn verify_slack_sig_accepts_valid_signature() {
        let secret = "top-secret";
        let timestamp = "1700000000";
        let body = br#"{"type":"event_callback"}"#;
        let base_string = format!("v0:{timestamp}:{}", String::from_utf8_lossy(body));
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(base_string.as_bytes());
        let digest = mac.finalize().into_bytes();
        let signature = format!("v0={}", hex_encode(digest.as_ref()));

        let mut headers = HeaderMap::new();
        headers.insert("X-Slack-Request-Timestamp", timestamp.parse().unwrap());
        headers.insert("X-Slack-Signature", signature.parse().unwrap());

        assert!(verify_slack_sig(secret, &headers, body));
    }

    #[test]
    fn verify_slack_sig_rejects_mismatch() {
        let mut headers = HeaderMap::new();
        headers.insert("X-Slack-Request-Timestamp", "1".parse().unwrap());
        headers.insert("X-Slack-Signature", "v0=deadbeef".parse().unwrap());
        assert!(!verify_slack_sig("secret", &headers, b"{}"));
    }

    #[test]
    fn map_slack_event_builds_envelope() {
        let event = SlackEvent {
            r#type: Some("message".into()),
            text: Some("hello".into()),
            user: Some("U123".into()),
            channel: Some("C456".into()),
            ts: Some("1700000000.000100".into()),
            thread_ts: Some("1700000000.000101".into()),
        };
        let (subject, envelope) = map_slack_event("acme", event).expect("event");
        assert_eq!(subject, "greentic.msg.in.acme.slack.C456");
        assert_eq!(envelope.chat_id, "C456");
        assert_eq!(envelope.user_id, "U123");
        assert_eq!(envelope.msg_id, "slack:1700000000.000100");
        assert_eq!(envelope.thread_id.as_deref(), Some("1700000000.000101"));
    }

    #[test]
    fn subtle_constant_time_eq_detects_difference() {
        assert!(subtle_constant_time_eq("abc", "abc"));
        assert!(!subtle_constant_time_eq("abc", "abd"));
        assert!(!subtle_constant_time_eq("abc", "ab"));
    }

    #[test]
    fn hex_encode_matches_expected_output() {
        assert_eq!(hex_encode(&[0xde, 0xad, 0xbe, 0xef]), "deadbeef");
    }
}
