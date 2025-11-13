use std::{collections::BTreeMap, str::FromStr, sync::Arc};

use async_nats::Client;
use axum::{
    Router, debug_handler,
    extract::{Extension, Json, Path},
    http::{HeaderMap, StatusCode},
    routing::post,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;

use crate::config::GatewayConfig;
use gsm_core::{MessageEnvelope, Platform, make_tenant_ctx};
use gsm_telemetry::set_current_tenant_ctx;

#[derive(Clone)]
pub struct GatewayState {
    pub client: Client,
    pub config: GatewayConfig,
}

impl GatewayState {
    fn subject(&self, tenant: &str, team: &str, channel: &str) -> String {
        format!(
            "greentic.messaging.ingress.{}.{}.{}.{}",
            self.config.env.0, tenant, team, channel
        )
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NormalizedRequest {
    pub chat_id: Option<String>,
    pub user_id: Option<String>,
    pub text: Option<String>,
    pub thread_id: Option<String>,
    pub msg_id: Option<String>,
    #[serde(default)]
    pub metadata: BTreeMap<String, Value>,
}

impl Default for NormalizedRequest {
    fn default() -> Self {
        Self {
            chat_id: None,
            user_id: None,
            text: None,
            thread_id: None,
            msg_id: None,
            metadata: BTreeMap::new(),
        }
    }
}

#[derive(Serialize)]
struct ApiResponse {
    status: String,
    subject: String,
}

#[derive(Serialize)]
struct ApiError {
    error: String,
}

pub async fn build_router(config: GatewayConfig) -> anyhow::Result<Router> {
    let client = async_nats::connect(&config.nats_url).await?;
    let state = Arc::new(GatewayState { client, config });

    let router = Router::new()
        .route("/api/:tenant/:channel", post(ingest_without_team))
        .route("/api/:tenant/:team/:channel", post(ingest_with_team))
        .layer(Extension(state));

    Ok(router)
}

#[debug_handler]
async fn ingest_without_team(
    Path((tenant, channel)): Path<(String, String)>,
    Extension(state): Extension<Arc<GatewayState>>,
    headers: HeaderMap,
    Json(payload): Json<NormalizedRequest>,
) -> Result<Json<ApiResponse>, (StatusCode, Json<ApiError>)> {
    handle_ingress(tenant, None, channel, state, payload, headers).await
}

#[debug_handler]
async fn ingest_with_team(
    Path((tenant, team, channel)): Path<(String, String, String)>,
    Extension(state): Extension<Arc<GatewayState>>,
    headers: HeaderMap,
    Json(payload): Json<NormalizedRequest>,
) -> Result<Json<ApiResponse>, (StatusCode, Json<ApiError>)> {
    handle_ingress(tenant, Some(team), channel, state, payload, headers).await
}

async fn handle_ingress(
    tenant: String,
    team_path: Option<String>,
    channel: String,
    state: Arc<GatewayState>,
    payload: NormalizedRequest,
    headers: HeaderMap,
) -> Result<Json<ApiResponse>, (StatusCode, Json<ApiError>)> {
    publish(
        &tenant,
        team_path.as_deref(),
        &channel,
        state.as_ref(),
        payload,
        &headers,
    )
    .await
    .map(|subject| {
        Json(ApiResponse {
            status: "accepted".into(),
            subject,
        })
    })
    .map_err(|(code, message)| (code, Json(ApiError { error: message })))
}

async fn publish(
    tenant: &str,
    team_path: Option<&str>,
    channel: &str,
    state: &GatewayState,
    payload: NormalizedRequest,
    headers: &HeaderMap,
) -> Result<String, (StatusCode, String)> {
    let chat_id = payload
        .chat_id
        .clone()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "chat_id is required".into()))?;
    let platform = Platform::from_str(channel).map_err(|err| {
        (
            StatusCode::BAD_REQUEST,
            format!("invalid platform name: {err}"),
        )
    })?;

    let team = sanitize_team(team_path, &state.config.default_team);
    let user_id = payload.user_id.or_else(|| {
        headers
            .get("x-greentic-user")
            .and_then(|v| v.to_str().ok().map(str::to_string))
    });

    let tenant_ctx = make_tenant_ctx(tenant.into(), Some(team.clone()), user_id.clone());
    set_current_tenant_ctx(tenant_ctx);

    let now = OffsetDateTime::now_utc();
    let timestamp = now
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| now.unix_timestamp().to_string());

    let msg_id = payload
        .msg_id
        .unwrap_or_else(|| format!("gw:{}", now.unix_timestamp_nanos()));

    let mut context = payload.metadata;
    if let Some(user) = user_id.as_deref() {
        context.insert("user".into(), Value::String(user.into()));
    }

    let envelope = MessageEnvelope {
        tenant: tenant.into(),
        platform,
        chat_id,
        user_id: user_id.unwrap_or_else(|| "unknown".into()),
        thread_id: payload.thread_id,
        msg_id,
        text: payload.text,
        timestamp,
        context,
    };

    let subject = state.subject(tenant, &team, envelope.platform.as_str());
    let payload_bytes = serde_json::to_vec(&envelope)
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;

    state
        .client
        .publish(subject.clone(), payload_bytes.into())
        .await
        .map_err(|err| (StatusCode::SERVICE_UNAVAILABLE, err.to_string()))?;

    Ok(subject)
}

fn sanitize_team(team: Option<&str>, default: &str) -> String {
    match team {
        Some(value) if !value.trim().is_empty() => value.trim().to_string(),
        _ => default.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::sanitize_team;

    #[test]
    fn sanitize_team_uses_default_when_missing() {
        assert_eq!(sanitize_team(None, "main"), "main");
    }

    #[test]
    fn sanitize_team_trims_values() {
        assert_eq!(sanitize_team(Some(" spy "), "main"), "spy");
    }
}
