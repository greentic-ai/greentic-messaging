use anyhow::Result;
use async_nats::Client as Nats;
use axum::{
    Extension, Json, Router,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
};
use gsm_core::*;
use gsm_dlq::{DlqError, DlqPublisher};
use gsm_idempotency::{IdKey as IdemKey, IdempotencyGuard};
use gsm_ingress_common::{
    SharedSessionStore, attach_session_id, init_guard, init_session_store, record_idempotency_hit,
    record_ingress, start_ingress_span,
};
use gsm_telemetry::{install as init_telemetry, set_current_tenant_ctx};
use security::middleware::{ActionContext, SharedActionContext, handle_action};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use time::OffsetDateTime;

#[derive(Clone)]
struct AppState {
    nats: Nats,
    tenant: String,
    idem_guard: IdempotencyGuard,
    dlq: DlqPublisher,
    sessions: SharedSessionStore,
}

#[tokio::main]
async fn main() -> Result<()> {
    init_telemetry("greentic-messaging")?;
    let nats_url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
    let tenant = std::env::var("TENANT").unwrap_or_else(|_| "acme".into());
    let nats = async_nats::connect(nats_url).await?;
    let idem_guard = init_guard(&nats).await?;
    let dlq = DlqPublisher::new("ingress", nats.clone()).await?;
    let sessions = init_session_store().await?;
    let state = AppState {
        nats,
        tenant,
        idem_guard,
        dlq,
        sessions,
    };

    let mut app = Router::new()
        .route("/teams/webhook", get(validate))
        .route("/teams/webhook", post(notify));

    match ActionContext::from_env(&state.nats).await {
        Ok(ctx) => {
            let shared: SharedActionContext = std::sync::Arc::new(ctx);
            app = app
                .route("/a", get(handle_action).layer(Extension(shared.clone())))
                .route("/a/teams", get(handle_action).layer(Extension(shared)));
        }
        Err(err) => {
            tracing::warn!(error = %err, "action links disabled for ingress-teams");
        }
    }

    let app = app.with_state(state);

    let addr: std::net::SocketAddr = std::env::var("BIND")
        .unwrap_or_else(|_| "0.0.0.0:8085".into())
        .parse()
        .unwrap();
    tracing::info!("ingress-teams listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

#[derive(Debug, Deserialize)]
struct ValidQs {
    #[serde(rename = "validationToken")]
    token: Option<String>,
}

async fn validate(Query(q): Query<ValidQs>) -> impl IntoResponse {
    if let Some(tok) = q.token {
        (axum::http::StatusCode::OK, tok)
    } else {
        (
            axum::http::StatusCode::BAD_REQUEST,
            "missing validationToken".to_string(),
        )
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct ChangeNotification {
    #[serde(rename = "subscriptionId")]
    subscription_id: String,
    #[serde(rename = "resource")]
    resource: String,
    #[serde(rename = "resourceData")]
    resource_data: Option<Value>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ChangeEnvelope {
    #[serde(rename = "value")]
    value: Vec<ChangeNotification>,
}

fn extract_chat_id(resource: &str) -> String {
    resource.split('/').nth(2).unwrap_or("unknown").to_string()
}

fn build_context(notif: &ChangeNotification) -> BTreeMap<String, Value> {
    let mut context = BTreeMap::new();
    context.insert(
        "subscription_id".into(),
        Value::String(notif.subscription_id.clone()),
    );
    context.insert("resource".into(), Value::String(notif.resource.clone()));
    context.insert(
        "resource_data".into(),
        notif.resource_data.clone().unwrap_or(Value::Null),
    );
    context
}

fn envelope_from_notification(tenant: &str, notif: &ChangeNotification) -> MessageEnvelope {
    let chat_id = extract_chat_id(&notif.resource);
    let msg_id = notif
        .resource_data
        .as_ref()
        .and_then(|rd| rd.get("id"))
        .and_then(|x| x.as_str())
        .unwrap_or("unknown")
        .to_string();

    MessageEnvelope {
        tenant: tenant.to_string(),
        platform: Platform::Teams,
        chat_id: chat_id.clone(),
        user_id: "unknown".into(),
        thread_id: None,
        msg_id: format!("teams:{msg_id}"),
        text: None,
        timestamp: OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_else(|_| "1970-01-01T00:00:00Z".into()),
        context: build_context(notif),
    }
}

async fn notify(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<Value>,
) -> axum::response::Response {
    if let Ok(expected) = std::env::var("INGRESS_BEARER") {
        let ok = headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|h| h.to_str().ok())
            .map(|value| value == format!("Bearer {expected}"))
            .unwrap_or(false);
        if !ok {
            tracing::warn!("missing or invalid bearer token");
            return StatusCode::UNAUTHORIZED.into_response();
        }
    }
    let env: ChangeEnvelope = match serde_json::from_value(payload.clone()) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("invalid change envelope: {e}");
            return StatusCode::BAD_REQUEST.into_response();
        }
    };

    for notif in env.value {
        let chat_id = extract_chat_id(&notif.resource);
        let envelope = envelope_from_notification(&state.tenant, &notif);
        let span = start_ingress_span(&envelope);
        let _guard = span.enter();
        let subject = in_subject(&state.tenant, envelope.platform.as_str(), &chat_id);
        let key = IdemKey {
            tenant: envelope.tenant.clone(),
            platform: envelope.platform.as_str().to_string(),
            msg_id: envelope.msg_id.clone(),
        };
        match state.idem_guard.should_process(&key).await {
            Ok(true) => {}
            Ok(false) => {
                record_idempotency_hit(&key.tenant);
                tracing::info!(
                    tenant = %key.tenant,
                    platform = %key.platform,
                    msg_id = %key.msg_id,
                    "duplicate teams event dropped"
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
        let mut invocation = match envelope.clone().into_invocation() {
            Ok(env) => env,
            Err(err) => {
                tracing::error!(error = %err, "failed to build invocation envelope");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        };

        let tenant_ctx = invocation.ctx.clone();
        attach_session_id(&state.sessions, &tenant_ctx, &envelope, &mut invocation).await;
        set_current_tenant_ctx(invocation.ctx.clone());

        match serde_json::to_vec(&invocation) {
            Ok(bytes) => {
                if let Err(e) = state.nats.publish(subject.clone(), bytes.into()).await {
                    tracing::error!("publish failed: {e}");
                    if let Err(dlq_err) = state
                        .dlq
                        .publish(
                            &state.tenant,
                            envelope.platform.as_str(),
                            &envelope.msg_id,
                            1,
                            DlqError {
                                code: "E_PUBLISH".into(),
                                message: e.to_string(),
                                stage: None,
                            },
                            &invocation,
                        )
                        .await
                    {
                        tracing::error!("failed to publish dlq entry: {dlq_err}");
                    }
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                } else {
                    record_ingress(&envelope);
                }
            }
            Err(e) => {
                tracing::error!("serialize envelope failed: {e}");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        }
        tracing::info!("published change for {chat_id}");
    }

    StatusCode::ACCEPTED.into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_notification() -> ChangeNotification {
        ChangeNotification {
            subscription_id: "sub-42".into(),
            resource: "/chats/chat-99/messages/123".into(),
            resource_data: Some(json!({ "id": "abc" })),
        }
    }

    #[test]
    fn extract_chat_id_parses_third_segment() {
        assert_eq!(extract_chat_id("/chats/chat-99/messages/123"), "chat-99");
        assert_eq!(extract_chat_id("invalid"), "unknown");
    }

    #[test]
    fn build_context_includes_subscription() {
        let ctx = build_context(&sample_notification());
        assert_eq!(
            ctx.get("subscription_id").unwrap(),
            &Value::String("sub-42".into())
        );
        assert!(ctx.contains_key("resource"));
    }

    #[test]
    fn envelope_from_notification_sets_fields() {
        let notif = sample_notification();
        let env = envelope_from_notification("tenant", &notif);
        assert_eq!(env.tenant, "tenant");
        assert_eq!(env.platform, Platform::Teams);
        assert_eq!(env.chat_id, "chat-99");
        assert_eq!(env.msg_id, "teams:abc");
    }
}
