//! Telegram ingress adapter: validates shared secrets, normalizes updates into
//! `MessageEnvelope`s, and publishes them to tenant-specific NATS subjects.
//!
//! ```text
//! Telegram POSTs updates to `/telegram/webhook`; the payload is deserialized
//! into `TelegramUpdate` and re-published to NATS.
//! ```

use anyhow::Result;
use async_nats::Client as Nats;
use axum::{
    extract::{Extension, State},
    middleware,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use gsm_core::*;
use gsm_dlq::{DlqError, DlqPublisher};
use gsm_idempotency::IdKey as IdemKey;
use gsm_ingress_common::{
    ack202, init_guard, rate_limit_layer, record_idempotency_hit, record_ingress,
    start_ingress_span, verify_bearer, verify_hmac, with_request_id,
};
use gsm_telemetry::{init_telemetry, TelemetryConfig};
use security::middleware::{handle_action, ActionContext, SharedActionContext};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;

#[derive(Clone)]
struct AppState {
    nats: Nats,
    tenant: String,
    secret_token: Option<String>,
    idem_guard: gsm_idempotency::IdempotencyGuard,
    dlq: DlqPublisher,
}

#[tokio::main]
async fn main() -> Result<()> {
    let telemetry = TelemetryConfig::from_env("gsm-ingress-telegram", env!("CARGO_PKG_VERSION"));
    init_telemetry(telemetry)?;

    let nats_url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
    let tenant = std::env::var("TENANT").unwrap_or_else(|_| "acme".into());
    let secret_token = std::env::var("TELEGRAM_SECRET_TOKEN").ok();
    let nats = async_nats::connect(nats_url).await?;
    let idem_guard = init_guard(&nats).await?;
    let dlq = DlqPublisher::new("ingress", nats.clone()).await?;
    let state = AppState {
        nats,
        tenant,
        secret_token,
        idem_guard,
        dlq,
    };

    let mut app = Router::new()
        .route("/telegram/webhook", post(handle_update))
        .layer(rate_limit_layer(20, 10))
        .layer(middleware::from_fn(with_request_id))
        .layer(middleware::from_fn(verify_bearer))
        .layer(middleware::from_fn(verify_hmac));

    match ActionContext::from_env(&state.nats).await {
        Ok(ctx) => {
            let shared: SharedActionContext = std::sync::Arc::new(ctx);
            app = app
                .route("/a", get(handle_action).layer(Extension(shared.clone())))
                .route("/a/telegram", get(handle_action).layer(Extension(shared)));
        }
        Err(err) => {
            tracing::warn!(error = %err, "action links disabled for ingress-telegram");
        }
    }

    let app = app.with_state(state);

    let addr: std::net::SocketAddr = std::env::var("BIND")
        .unwrap_or_else(|_| "0.0.0.0:8080".into())
        .parse()
        .unwrap();
    tracing::info!("ingress-telegram listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct TelegramUpdate {
    update_id: i64,
    #[serde(default)]
    message: Option<TelegramMessage>,
    #[serde(default)]
    edited_message: Option<TelegramMessage>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct TelegramMessage {
    message_id: i64,
    date: i64,
    #[serde(default)]
    text: Option<String>,
    chat: TelegramChat,
    from: Option<TelegramUser>,
    #[serde(default)]
    reply_to_message: Option<Box<ReplyMessageRef>>,
    #[serde(default)]
    message_thread_id: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct ReplyMessageRef {
    message_id: i64,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct TelegramChat {
    id: i64,
    #[serde(default)]
    r#type: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct TelegramUser {
    id: i64,
    username: Option<String>,
}

fn secret_token_valid(expected: &Option<String>, provided: Option<&str>) -> bool {
    match expected {
        Some(exp) => provided == Some(exp.as_str()),
        None => true,
    }
}

fn extract_message(update: &TelegramUpdate) -> Option<&TelegramMessage> {
    update.message.as_ref().or(update.edited_message.as_ref())
}

fn envelope_from_message(tenant: &str, msg: &TelegramMessage) -> MessageEnvelope {
    let chat_id = msg.chat.id.to_string();
    let user_id = msg
        .from
        .as_ref()
        .map(|u| u.id.to_string())
        .unwrap_or_else(|| "unknown".into());
    let thread_id = msg
        .reply_to_message
        .as_ref()
        .map(|reply| reply.message_id.to_string())
        .or_else(|| msg.message_thread_id.map(|id| id.to_string()));
    let ts = OffsetDateTime::from_unix_timestamp(msg.date as i64)
        .unwrap_or_else(|_| OffsetDateTime::now_utc())
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".into());

    MessageEnvelope {
        tenant: tenant.to_string(),
        platform: Platform::Telegram,
        chat_id: chat_id.clone(),
        user_id,
        thread_id,
        msg_id: format!("tg:{}", msg.message_id),
        text: msg.text.clone(),
        timestamp: ts,
        context: Default::default(),
    }
}

async fn handle_update(
    State(state): State<AppState>,
    request_id: Option<Extension<String>>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<Value>,
) -> axum::response::Response {
    let provided_token = headers
        .get("X-Telegram-Bot-Api-Secret-Token")
        .and_then(|v| v.to_str().ok());
    if !secret_token_valid(&state.secret_token, provided_token) {
        tracing::warn!("telegram secret token mismatch");
        return axum::http::StatusCode::UNAUTHORIZED.into_response();
    }

    let update: TelegramUpdate = match serde_json::from_value(payload.clone()) {
        Ok(u) => u,
        Err(e) => {
            tracing::warn!("bad update: {e}");
            return axum::http::StatusCode::BAD_REQUEST.into_response();
        }
    };

    if let Some(msg) = extract_message(&update).cloned() {
        let env = envelope_from_message(&state.tenant, &msg);
        let span = start_ingress_span(&env);
        let _guard = span.enter();
        let id_key = IdemKey {
            tenant: env.tenant.clone(),
            platform: env.platform.as_str().to_string(),
            msg_id: env.msg_id.clone(),
        };
        match state.idem_guard.should_process(&id_key).await {
            Ok(true) => {}
            Ok(false) => {
                record_idempotency_hit(&id_key.tenant);
                let rid_ref = request_id.as_ref().map(|Extension(id)| id);
                tracing::info!(
                    tenant = %id_key.tenant,
                    platform = %id_key.platform,
                    msg_id = %id_key.msg_id,
                    "duplicate telegram event dropped"
                );
                return ack202(rid_ref).into_response();
            }
            Err(err) => {
                tracing::error!(
                    error = %err,
                    tenant = %id_key.tenant,
                    platform = %id_key.platform,
                    msg_id = %id_key.msg_id,
                    "idempotency check failed; continuing"
                );
            }
        }
        let subject = in_subject(&state.tenant, env.platform.as_str(), &env.chat_id);
        if let Ok(bytes) = serde_json::to_vec(&env) {
            if let Err(e) = state.nats.publish(subject.clone(), bytes.into()).await {
                tracing::error!("publish failed: {e}");
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
                return axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response();
            } else {
                tracing::info!("published to {subject}");
                record_ingress(&env);
            }
        }

        let rid_ref = request_id.as_ref().map(|Extension(id)| id);
        return ack202(rid_ref).into_response();
    }

    axum::http::StatusCode::OK.into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_message() -> TelegramMessage {
        TelegramMessage {
            message_id: 42,
            date: 1_700_000_000,
            text: Some("Hello".into()),
            chat: TelegramChat {
                id: 123,
                r#type: Some("private".into()),
            },
            from: Some(TelegramUser {
                id: 99,
                username: Some("bot".into()),
            }),
            reply_to_message: None,
            message_thread_id: None,
        }
    }

    #[test]
    fn extract_message_prefers_new_message() {
        let msg = sample_message();
        let update = TelegramUpdate {
            update_id: 1,
            message: Some(msg.clone()),
            edited_message: Some(msg),
        };
        let selected = extract_message(&update).unwrap();
        assert_eq!(selected.message_id, 42);
    }

    #[test]
    fn envelope_from_message_maps_fields() {
        let msg = sample_message();
        let env = envelope_from_message("tenant", &msg);
        assert_eq!(env.tenant, "tenant");
        assert_eq!(env.chat_id, "123");
        assert_eq!(env.user_id, "99");
        assert_eq!(env.msg_id, "tg:42");
        assert_eq!(env.text.as_deref(), Some("Hello"));
        assert!(env.thread_id.is_none());
    }

    #[test]
    fn envelope_includes_reply_to_message() {
        let mut msg = sample_message();
        msg.reply_to_message = Some(Box::new(ReplyMessageRef { message_id: 21 }));
        let env = envelope_from_message("tenant", &msg);
        assert_eq!(env.thread_id.as_deref(), Some("21"));
    }

    #[test]
    fn secret_token_validates_values() {
        let expected = Some("secret".to_string());
        assert!(secret_token_valid(&expected, Some("secret")));
        assert!(!secret_token_valid(&expected, Some("wrong")));
        assert!(!secret_token_valid(&expected, None));
        assert!(secret_token_valid(&None, None));
    }
}
