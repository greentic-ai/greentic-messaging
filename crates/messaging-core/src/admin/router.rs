use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, Query, RawQuery, State},
    http::StatusCode,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};

use super::{
    AdminRegistry,
    models::{DesiredGlobalApp, DesiredTenantBinding, ProvisionReport},
    plan::{validate_global_app, validate_tenant_binding},
    traits::{AdminError, GlobalProvisioner, TenantProvisioner},
};

#[derive(Clone)]
struct AdminRouterState {
    registry: Arc<AdminRegistry>,
}

#[derive(Serialize)]
struct ProviderCapsResponse {
    provider: String,
    caps: super::models::ProvisionCaps,
}

#[derive(Serialize)]
struct StartResponse {
    started: bool,
    redirect_url: Option<String>,
}

#[derive(Serialize)]
struct ApiErrorBody {
    error: String,
    message: String,
}

type ApiErrorReply = (StatusCode, Json<ApiErrorBody>);

type ApiResult<T> = Result<Json<T>, ApiErrorReply>;

#[derive(Debug, Deserialize)]
struct TenantStartQuery {
    tenant_key: String,
    provider_tenant_id: String,
}

pub fn admin_router(registry: Arc<AdminRegistry>) -> Router {
    let state = AdminRouterState { registry };

    Router::new()
        .route("/providers", get(list_providers))
        .route("/{provider}/global/ensure", post(ensure_global))
        .route("/{provider}/global/start", post(start_global))
        .route("/{provider}/global/callback", get(global_callback))
        .route("/{provider}/tenant/plan", post(plan_tenant))
        .route("/{provider}/tenant/ensure", post(ensure_tenant))
        .route("/{provider}/tenant/start", post(start_tenant))
        .route("/{provider}/tenant/callback", get(tenant_callback))
        .with_state(state)
}

async fn list_providers(
    State(state): State<AdminRouterState>,
) -> ApiResult<Vec<ProviderCapsResponse>> {
    let mut providers: Vec<_> = state
        .registry
        .globals
        .iter()
        .map(|(name, prov)| ProviderCapsResponse {
            provider: (*name).to_string(),
            caps: prov.capabilities(),
        })
        .collect();
    providers.sort_by(|a, b| a.provider.cmp(&b.provider));

    Ok(Json(providers))
}

async fn ensure_global(
    Path(provider): Path<String>,
    State(state): State<AdminRouterState>,
    Json(payload): Json<DesiredGlobalApp>,
) -> ApiResult<ProvisionReport> {
    validate_global_app(&payload).map_err(map_error)?;
    let prov = get_global(&state, &provider)?;
    let report = prov.ensure_global(&payload).await.map_err(map_error)?;
    Ok(Json(report))
}

async fn start_global(
    Path(provider): Path<String>,
    State(state): State<AdminRouterState>,
) -> ApiResult<StartResponse> {
    let prov = get_global(&state, &provider)?;
    let redirect = prov.start_global_consent().await.map_err(map_error)?;
    Ok(Json(StartResponse {
        started: redirect.is_some(),
        redirect_url: redirect.map(|u| u.to_string()),
    }))
}

async fn global_callback(
    Path(provider): Path<String>,
    State(state): State<AdminRouterState>,
    RawQuery(raw): RawQuery,
) -> Result<StatusCode, ApiErrorReply> {
    let prov = get_global(&state, &provider)?;
    let pairs = parse_query(raw);
    prov.handle_global_callback(&pairs)
        .await
        .map_err(map_error)?;
    Ok(StatusCode::OK)
}

async fn plan_tenant(
    Path(provider): Path<String>,
    State(state): State<AdminRouterState>,
    Json(payload): Json<DesiredTenantBinding>,
) -> ApiResult<ProvisionReport> {
    validate_tenant_binding(&payload).map_err(map_error)?;
    let tenant = get_tenant(&state, &provider)?;
    let report = tenant.plan_tenant(&payload).await.map_err(map_error)?;
    Ok(Json(report))
}

async fn ensure_tenant(
    Path(provider): Path<String>,
    State(state): State<AdminRouterState>,
    Json(payload): Json<DesiredTenantBinding>,
) -> ApiResult<ProvisionReport> {
    validate_tenant_binding(&payload).map_err(map_error)?;
    let tenant = get_tenant(&state, &provider)?;
    let report = tenant.ensure_tenant(&payload).await.map_err(map_error)?;
    Ok(Json(report))
}

async fn start_tenant(
    Path(provider): Path<String>,
    State(state): State<AdminRouterState>,
    Query(query): Query<TenantStartQuery>,
) -> ApiResult<StartResponse> {
    let tenant = get_tenant(&state, &provider)?;
    let redirect = tenant
        .start_tenant_consent(&query.tenant_key, &query.provider_tenant_id)
        .await
        .map_err(map_error)?;
    Ok(Json(StartResponse {
        started: redirect.is_some(),
        redirect_url: redirect.map(|u| u.to_string()),
    }))
}

async fn tenant_callback(
    Path(provider): Path<String>,
    State(state): State<AdminRouterState>,
    RawQuery(raw): RawQuery,
) -> Result<StatusCode, ApiErrorReply> {
    let tenant = get_tenant(&state, &provider)?;
    let pairs = parse_query(raw);
    let tenant_key = pairs
        .iter()
        .find(|(k, _)| k == "tenant_key")
        .map(|(_, v)| v.clone())
        .ok_or_else(missing_tenant_key)?;
    tenant
        .handle_tenant_callback(&tenant_key, &pairs)
        .await
        .map_err(map_error)?;
    Ok(StatusCode::OK)
}

fn get_global(
    state: &AdminRouterState,
    provider: &str,
) -> Result<Arc<dyn GlobalProvisioner>, ApiErrorReply> {
    state
        .registry
        .global(provider)
        .ok_or_else(|| not_found(provider, "global provisioner"))
}

fn get_tenant(
    state: &AdminRouterState,
    provider: &str,
) -> Result<Arc<dyn TenantProvisioner>, ApiErrorReply> {
    state
        .registry
        .tenant(provider)
        .ok_or_else(|| not_found(provider, "tenant provisioner"))
}

fn not_found(provider: &str, kind: &str) -> ApiErrorReply {
    (
        StatusCode::NOT_FOUND,
        Json(ApiErrorBody {
            error: "not_found".into(),
            message: format!("no {kind} registered for provider {provider}"),
        }),
    )
}

fn missing_tenant_key() -> ApiErrorReply {
    (
        StatusCode::BAD_REQUEST,
        Json(ApiErrorBody {
            error: "validation_error".into(),
            message: "missing tenant_key".into(),
        }),
    )
}

fn map_error(err: AdminError) -> ApiErrorReply {
    let (status, error, message) = match err {
        AdminError::Validation(msg) => (StatusCode::BAD_REQUEST, "validation_error", msg),
        AdminError::Unauthorized => (
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "unauthorized".to_string(),
        ),
        AdminError::NotFound => (
            StatusCode::NOT_FOUND,
            "not_found",
            "resource not found".to_string(),
        ),
        AdminError::Conflict(msg) => (StatusCode::CONFLICT, "conflict", msg),
        AdminError::Provider(msg) => (StatusCode::BAD_GATEWAY, "provider_error", msg),
        AdminError::Secrets(msg) => (StatusCode::BAD_GATEWAY, "secrets_error", msg),
        AdminError::Internal { .. } => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "internal error".to_string(),
        ),
    };

    (
        status,
        Json(ApiErrorBody {
            error: error.into(),
            message,
        }),
    )
}

fn parse_query(raw: Option<String>) -> Vec<(String, String)> {
    raw.map(|query| {
        url::form_urlencoded::parse(query.as_bytes())
            .into_owned()
            .collect()
    })
    .unwrap_or_default()
}
