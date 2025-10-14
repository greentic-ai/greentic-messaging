//! WebChat ingress service that accepts webhook posts and forwards normalized
//! `MessageEnvelope`s onto NATS, mirroring the behaviour of other ingress adapters.
//!
//! ```text
//! POST `{ "chat_id": "chat-1", "user_id": "user-42", "text": "hi" }`
//! to `/webhook` while the service is running to publish to NATS.
//! ```

use anyhow::Result;
use async_nats::Client as Nats;
use axum::{
    extract::{Extension, State},
    middleware,
    response::{Html, IntoResponse},
    routing::{get, post},
    Json, Router,
};
use gsm_core::*;
use gsm_ingress_common::{ack202, verify_bearer, with_request_id};
use include_dir::{include_dir, Dir};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use tracing_subscriber::EnvFilter;

static ASSETS: Dir = include_dir!("$CARGO_MANIFEST_DIR/static");

#[derive(Clone)]
struct AppState {
    nats: Nats,
    tenant: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let nats_url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
    let tenant = std::env::var("TENANT").unwrap_or_else(|_| "acme".into());
    let nats = async_nats::connect(nats_url).await?;
    let state = AppState { nats, tenant };

    let app = Router::new()
        .route("/", get(index))
        .route("/webhook", post(webhook))
        .with_state(state)
        .layer(middleware::from_fn(with_request_id))
        .layer(middleware::from_fn(verify_bearer));

    let addr: std::net::SocketAddr = std::env::var("BIND")
        .unwrap_or_else(|_| "0.0.0.0:8090".into())
        .parse()
        .unwrap();
    tracing::info!("ingress-webchat listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn index() -> Html<String> {
    let file = ASSETS.get_file("index.html").expect("index.html missing");
    Html(String::from_utf8_lossy(file.contents()).to_string())
}

#[derive(Debug, Deserialize, Serialize)]
struct WebMsg {
    chat_id: String,
    user_id: String,
    text: String,
}

fn envelope_from_webmsg(tenant: &str, msg: &WebMsg, now: OffsetDateTime) -> MessageEnvelope {
    MessageEnvelope {
        tenant: tenant.to_string(),
        platform: Platform::WebChat,
        chat_id: msg.chat_id.clone(),
        user_id: msg.user_id.clone(),
        thread_id: None,
        msg_id: format!("web:{}", now.unix_timestamp_nanos()),
        text: Some(msg.text.clone()),
        timestamp: now
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_else(|_| "1970-01-01T00:00:00Z".into()),
        context: Default::default(),
    }
}

async fn webhook(
    request_id: Option<Extension<String>>,
    State(state): State<AppState>,
    Json(msg): Json<WebMsg>,
) -> axum::response::Response {
    let now = OffsetDateTime::now_utc();
    let env = envelope_from_webmsg(&state.tenant, &msg, now);

    let subject = in_subject(&state.tenant, env.platform.as_str(), &env.chat_id);

    match serde_json::to_vec(&env) {
        Ok(bytes) => {
            if let Err(e) = state.nats.publish(subject.clone(), bytes.into()).await {
                tracing::error!("publish failed: {e}");
                return axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            tracing::info!("published to {subject}");
        }
        Err(e) => {
            tracing::error!("serialize failed: {e}");
            return axum::http::StatusCode::BAD_REQUEST.into_response();
        }
    }

    let rid = request_id.as_ref().map(|Extension(id)| id);
    ack202(rid).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_from_webmsg_sets_fields() {
        let msg = WebMsg {
            chat_id: "chat-1".into(),
            user_id: "user-2".into(),
            text: "hi".into(),
        };
        let now = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        let env = envelope_from_webmsg("tenant", &msg, now);
        assert_eq!(env.tenant, "tenant");
        assert_eq!(env.chat_id, "chat-1");
        assert_eq!(env.user_id, "user-2");
        assert_eq!(env.text.as_deref(), Some("hi"));
        assert!(env.msg_id.starts_with("web:"));
    }

    #[test]
    fn envelope_timestamp_is_rfc3339() {
        let msg = WebMsg {
            chat_id: "chat-1".into(),
            user_id: "user-2".into(),
            text: "hi".into(),
        };
        let now = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        let env = envelope_from_webmsg("tenant", &msg, now);
        assert_eq!(env.timestamp, "2023-11-14T22:13:20Z");
    }

    #[test]
    fn webmsg_serializes_roundtrip() {
        let original = WebMsg {
            chat_id: "chat".into(),
            user_id: "user".into(),
            text: "hello".into(),
        };
        let json = serde_json::to_string(&original).unwrap();
        let parsed: WebMsg = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.chat_id, "chat");
        assert_eq!(parsed.user_id, "user");
        assert_eq!(parsed.text, "hello");
    }
}
