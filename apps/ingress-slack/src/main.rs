//! Slack Events API ingress service.
//!
//! Exposes a `/slack/events` endpoint that validates the Slack signing secret,
//! acknowledges URL verification challenges, and forwards normalized messages to NATS.

use anyhow::Result;
use async_nats::Client as Nats;
use axum::{
    Extension, Router,
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use gsm_core::{
    InvocationEnvelope, MessageEnvelope, Platform, TenantCtx, in_subject, make_tenant_ctx,
};
use gsm_dlq::{DlqError, DlqPublisher};
use gsm_idempotency::{IdKey as IdemKey, IdempotencyGuard};
use gsm_ingress_common::{
    SharedSessionStore, attach_session_id, init_guard, init_session_store, record_idempotency_hit,
    record_ingress, start_ingress_span,
};
use gsm_telemetry::{install as init_telemetry, set_current_tenant_ctx};
use hmac::{Hmac, Mac};
use security::middleware::{ActionContext, SharedActionContext, handle_action};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use time::OffsetDateTime;

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone)]
struct AppState {
    nats: Nats,
    signing_secret: String,
    idem_guard: IdempotencyGuard,
    dlq: DlqPublisher,
    bot_user_id: Option<String>,
    sessions: SharedSessionStore,
}

#[derive(Debug, Deserialize)]
struct SlackPath {
    tenant: String,
    team: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    init_telemetry("greentic-messaging")?;
    let nats_url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
    let signing_secret =
        std::env::var("SLACK_SIGNING_SECRET").expect("SLACK_SIGNING_SECRET required");
    let bot_user_id = std::env::var("SLACK_BOT_USER_ID").ok();
    let nats = async_nats::connect(nats_url).await?;
    let idem_guard = init_guard(&nats).await?;
    let dlq = DlqPublisher::new("ingress", nats.clone()).await?;
    let sessions = init_session_store().await?;
    let state = AppState {
        nats,
        signing_secret,
        idem_guard,
        dlq,
        bot_user_id,
        sessions,
    };

    let mut app = Router::new().route("/ingress/slack/:tenant/:team/events", post(handle));

    match ActionContext::from_env(&state.nats).await {
        Ok(ctx) => {
            let shared: SharedActionContext = std::sync::Arc::new(ctx);
            app = app
                .route("/a", get(handle_action).layer(Extension(shared.clone())))
                .route("/a/slack", get(handle_action).layer(Extension(shared)));
        }
        Err(err) => {
            tracing::warn!(error = %err, "action links disabled for ingress-slack");
        }
    }

    let app = app.with_state(state);

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

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Default)]
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
    #[serde(default)]
    bot_id: Option<String>,
    #[serde(default)]
    subtype: Option<String>,
}

async fn handle(
    State(state): State<AppState>,
    Path(path): Path<SlackPath>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
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
        let ctx = tenant_ctx_from_path(&path);
        match slack_event_invocation(&ctx, state.bot_user_id.as_deref(), event) {
            Some((subject, message, mut invocation)) => {
                attach_session_id(&state.sessions, &ctx, &message, &mut invocation).await;
                set_current_tenant_ctx(invocation.ctx.clone());
                let span = start_ingress_span(&message);
                let _guard = span.enter();
                let key = IdemKey {
                    tenant: ctx.tenant.as_str().to_string(),
                    platform: message.platform.as_str().to_string(),
                    msg_id: message.msg_id.clone(),
                };
                match state.idem_guard.should_process(&key).await {
                    Ok(true) => {}
                    Ok(false) => {
                        record_idempotency_hit(&key.tenant);
                        tracing::info!(
                            tenant = %key.tenant,
                            platform = %key.platform,
                            msg_id = %key.msg_id,
                            "duplicate slack event dropped"
                        );
                        return StatusCode::OK.into_response();
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

                let payload = match serde_json::to_vec(&invocation) {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        tracing::error!("serialize invocation failed: {error}");
                        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                    }
                };

                if let Err(error) = state.nats.publish(subject.clone(), payload.into()).await {
                    tracing::error!("publish failed on {subject}: {error}");
                    if let Err(dlq_err) = state
                        .dlq
                        .publish(
                            ctx.tenant.as_str(),
                            message.platform.as_str(),
                            &message.msg_id,
                            1,
                            DlqError {
                                code: "E_PUBLISH".into(),
                                message: error.to_string(),
                                stage: None,
                            },
                            &invocation,
                        )
                        .await
                    {
                        tracing::error!("failed to publish dlq entry: {dlq_err}");
                    }
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }

                tracing::info!("published slack event for {}", message.chat_id);
                record_ingress(&message);
                return StatusCode::OK.into_response();
            }
            None => {
                tracing::debug!("ignored slack event");
                return StatusCode::OK.into_response();
            }
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

fn tenant_ctx_from_path(path: &SlackPath) -> TenantCtx {
    let team = normalize_team(&path.team);
    make_tenant_ctx(path.tenant.clone(), team, None)
}

fn normalize_team(team: &str) -> Option<String> {
    let trimmed = team.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("default") {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn slack_event_invocation(
    ctx: &TenantCtx,
    bot_user_id: Option<&str>,
    event: SlackEvent,
) -> Option<(String, MessageEnvelope, InvocationEnvelope)> {
    let SlackEvent {
        r#type,
        text,
        user,
        channel,
        ts,
        thread_ts,
        bot_id,
        subtype,
    } = event;

    if let Some(bot) = bot_id {
        tracing::debug!(bot_id = %bot, "ignoring slack bot event");
        return None;
    }

    if matches!(
        subtype.as_deref(),
        Some("bot_message" | "message_changed" | "message_deleted")
    ) {
        tracing::debug!(?subtype, "ignoring slack subtype event");
        return None;
    }

    if let Some(kind) = r#type.as_deref()
        && kind != "message"
    {
        tracing::debug!(event_type = ?kind, "ignoring non-message slack event");
        return None;
    }

    let chat_id = channel?;
    let user_id = user?;
    if let Some(bot_user) = bot_user_id
        && bot_user == user_id
        && text.as_deref().map(|t| t.trim().is_empty()).unwrap_or(true)
    {
        tracing::debug!(user_id = %user_id, "ignoring slack event from bot user (empty text)");
        return None;
    }

    let timestamp = ts.unwrap_or_else(|| "0".into());
    let now = OffsetDateTime::now_utc();
    let envelope = MessageEnvelope {
        tenant: ctx.tenant.as_str().to_string(),
        platform: Platform::Slack,
        chat_id: chat_id.clone(),
        user_id,
        thread_id: thread_ts,
        msg_id: format!("slack:{timestamp}"),
        text,
        timestamp: now
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_else(|_| "1970-01-01T00:00:00Z".into()),
        context: Default::default(),
    };

    let mut invocation = match envelope.clone().into_invocation() {
        Ok(env) => env,
        Err(err) => {
            tracing::error!(error = %err, "failed to build slack invocation");
            return None;
        }
    };
    invocation.ctx = ctx.clone();

    if !matches!(r#type.as_deref(), Some("message")) {
        invocation.op = "on_event".into();
    }

    let subject = in_subject(ctx.tenant.as_str(), Platform::Slack.as_str(), &chat_id);
    Some((subject, envelope, invocation))
}

/// Verifies Slack's signed request using the signing secret.
///
/// ```
/// use axum::http::HeaderMap;
/// use gsm_ingress_slack::verify_slack_sig;
///
/// let mut headers = HeaderMap::new();
/// headers.insert("X-Slack-Request-Timestamp", "1".parse().unwrap());
/// headers.insert("X-Slack-Signature", "v0=<signature>".parse().unwrap());
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
    use rand::RngCore;

    fn random_secret() -> String {
        let mut buf = [0u8; 32];
        let mut rng = rand::rng();
        rng.fill_bytes(&mut buf);
        hex_encode(buf.as_ref())
    }

    #[test]
    fn verify_slack_sig_accepts_valid_signature() {
        let secret = random_secret();
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

        assert!(verify_slack_sig(&secret, &headers, body));
    }

    #[test]
    fn verify_slack_sig_rejects_mismatch() {
        let good_secret = random_secret();
        let bad_secret = random_secret();
        let timestamp = "1700000001";
        let body = br#"{"type":"event_callback"}"#;
        let base_string = format!("v0:{timestamp}:{}", String::from_utf8_lossy(body));
        let mut mac = HmacSha256::new_from_slice(good_secret.as_bytes()).unwrap();
        mac.update(base_string.as_bytes());
        let digest = mac.finalize().into_bytes();
        let signature = format!("v0={}", hex_encode(digest.as_ref()));

        let mut headers = HeaderMap::new();
        headers.insert("X-Slack-Request-Timestamp", timestamp.parse().unwrap());
        headers.insert("X-Slack-Signature", signature.parse().unwrap());
        assert!(!verify_slack_sig(&bad_secret, &headers, body));
    }

    #[test]
    fn slack_event_invocation_builds_envelope() {
        let event = SlackEvent {
            r#type: Some("message".into()),
            text: Some("hello".into()),
            user: Some("U123".into()),
            channel: Some("C456".into()),
            ts: Some("1700000000.000100".into()),
            thread_ts: Some("1700000000.000101".into()),
            ..Default::default()
        };
        let ctx = make_tenant_ctx("acme".into(), Some("team".into()), None);
        let (subject, envelope, invocation) =
            slack_event_invocation(&ctx, None, event).expect("event");
        assert_eq!(subject, "greentic.msg.in.acme.slack.C456");
        assert_eq!(envelope.chat_id, "C456");
        assert_eq!(envelope.user_id, "U123");
        assert_eq!(envelope.msg_id, "slack:1700000000.000100");
        assert_eq!(envelope.thread_id.as_deref(), Some("1700000000.000101"));
        assert_eq!(invocation.ctx.tenant.as_str(), "acme");
    }

    #[test]
    fn slack_event_invocation_skips_bot_messages() {
        let bot_event = SlackEvent {
            r#type: Some("message".into()),
            text: Some("bot message".into()),
            user: None,
            channel: Some("C456".into()),
            ts: Some("1700000000.000200".into()),
            bot_id: Some("B999".into()),
            ..Default::default()
        };
        let ctx = make_tenant_ctx("acme".into(), None, None);
        assert!(slack_event_invocation(&ctx, None, bot_event).is_none());

        let subtype_event = SlackEvent {
            r#type: Some("message".into()),
            text: Some("bot subtype".into()),
            user: Some("Ubot".into()),
            channel: Some("C456".into()),
            ts: Some("1700000000.000300".into()),
            subtype: Some("bot_message".into()),
            ..Default::default()
        };
        assert!(slack_event_invocation(&ctx, None, subtype_event).is_none());

        let bot_user_event = SlackEvent {
            r#type: Some("message".into()),
            text: None,
            user: Some("Ubot".into()),
            channel: Some("C456".into()),
            ts: Some("1700000000.000400".into()),
            ..Default::default()
        };
        assert!(slack_event_invocation(&ctx, Some("Ubot"), bot_user_event).is_none());
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
