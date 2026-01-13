use std::{collections::BTreeMap, str::FromStr, sync::Arc};

use axum::{
    Router, debug_handler,
    extract::{Extension, Json, Path},
    http::{HeaderMap, StatusCode},
    routing::post,
};
use metrics::counter;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;
use tracing::{Instrument, warn};

use crate::config::GatewayConfig;
use gsm_bus::{BusClient, BusError, to_value};
use gsm_core::{
    AdapterDescriptor, AdapterRegistry, ChannelMessage, Platform, ProviderExtensionsRegistry,
    WorkerClient, WorkerRoutingConfig, forward_to_worker, infer_platform_from_adapter_name,
    make_tenant_ctx,
};
use gsm_telemetry::set_current_tenant_ctx;

#[derive(Clone)]
pub struct GatewayState {
    pub bus: Arc<dyn BusClient>,
    pub config: GatewayConfig,
    pub adapters: AdapterRegistry,
    pub provider_extensions: ProviderExtensionsRegistry,
    pub workers: BTreeMap<String, Arc<dyn WorkerClient>>,
    pub worker_default: Option<WorkerRoutingConfig>,
    pub worker_egress_subject: Option<String>,
}

impl GatewayState {
    fn subject(&self, tenant: &str, team: &str, platform: &str) -> String {
        gsm_bus::ingress_subject_with_prefix(
            self.config.subject_prefix.as_str(),
            self.config.env.0.as_str(),
            tenant,
            team,
            platform,
        )
    }
}

#[derive(Debug, Deserialize, Serialize, Default)]
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

#[derive(Serialize, Debug)]
pub struct ApiResponse {
    status: String,
    subject: String,
}

#[derive(Serialize, Debug)]
pub struct ApiError {
    error: String,
}

fn outbound_to_out_message(
    outbound: gsm_core::OutboundEnvelope,
    platform: Platform,
    thread_id: Option<String>,
) -> gsm_core::OutMessage {
    let mut meta = std::collections::BTreeMap::new();
    let kind = outbound
        .meta
        .as_object()
        .and_then(|m| m.get("kind"))
        .and_then(|v| v.as_str())
        .unwrap_or("text");
    let text = outbound
        .body
        .get("text")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| outbound.body.as_str().map(|s| s.to_string()))
        .or_else(|| Some(outbound.body.to_string()));
    if let Some(obj) = outbound.meta.as_object() {
        for (k, v) in obj {
            meta.insert(k.clone(), v.clone());
        }
    }
    meta.insert("worker_payload".into(), outbound.body.clone());

    gsm_core::OutMessage {
        ctx: outbound.tenant.clone(),
        tenant: outbound.tenant.tenant.as_str().to_string(),
        platform,
        chat_id: outbound.session_id.clone(),
        thread_id,
        kind: if kind.eq_ignore_ascii_case("card") {
            gsm_core::OutKind::Card
        } else {
            gsm_core::OutKind::Text
        },
        text: if kind.eq_ignore_ascii_case("card") {
            None
        } else {
            text
        },
        message_card: None,
        adaptive_card: None,
        meta,
    }
}

pub async fn build_router_with_bus(
    config: GatewayConfig,
    adapters: AdapterRegistry,
    provider_extensions: ProviderExtensionsRegistry,
    bus: Arc<dyn BusClient>,
    workers: BTreeMap<String, Arc<dyn WorkerClient>>,
) -> anyhow::Result<Router> {
    let state = Arc::new(GatewayState {
        bus,
        worker_default: config.worker_routing.clone(),
        worker_egress_subject: config.worker_egress_subject.clone(),
        config,
        adapters,
        provider_extensions,
        workers,
    });

    if state.adapters.is_empty() {
        warn!("gsm-gateway running with no adapter packs; legacy platform names only");
    }

    let router = Router::new()
        .route("/api/{tenant}/{channel}", post(ingest_without_team))
        .route("/api/{tenant}/{team}/{channel}", post(ingest_with_team))
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

pub async fn handle_ingress(
    tenant: String,
    team_path: Option<String>,
    channel: String,
    state: Arc<GatewayState>,
    payload: NormalizedRequest,
    headers: HeaderMap,
) -> Result<Json<ApiResponse>, (StatusCode, Json<ApiError>)> {
    let span = tracing::info_span!(
        "ingress",
        tenant = %tenant,
        team = team_path.as_deref().unwrap_or(""),
        channel = %channel
    );
    async move {
        let (platform, adapter) = resolve_ingress_target(&channel, &state.adapters)
            .map_err(|(code, message)| (code, Json(ApiError { error: message })))?;

        publish(
            &tenant,
            team_path.as_deref(),
            &platform,
            adapter.as_ref(),
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
    .instrument(span)
    .await
}

async fn publish(
    tenant: &str,
    team_path: Option<&str>,
    platform: &Platform,
    adapter: Option<&AdapterDescriptor>,
    state: &GatewayState,
    payload: NormalizedRequest,
    headers: &HeaderMap,
) -> Result<String, (StatusCode, String)> {
    let chat_id = payload
        .chat_id
        .clone()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "chat_id is required".into()))?;

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
    if let Some(adapter) = adapter {
        context.insert("adapter".into(), Value::String(adapter.name.clone()));
    }
    if let Some(user) = user_id.as_deref() {
        context.insert("user".into(), Value::String(user.into()));
    }

    let tenant_ctx = make_tenant_ctx(tenant.into(), Some(team.clone()), user_id.clone());
    let session_id = payload.thread_id.clone().unwrap_or_else(|| chat_id.clone());
    let route = adapter.map(|a| a.name.clone());
    let envelope = ChannelMessage {
        tenant: tenant_ctx,
        channel_id: platform.as_str().to_string(),
        session_id,
        route,
        payload: serde_json::json!({
            "chat_id": chat_id,
            "user_id": user_id,
            "thread_id": payload.thread_id,
            "msg_id": msg_id,
            "text": payload.text,
            "timestamp": timestamp,
            "metadata": context,
            "headers": headers_to_json(headers),
        }),
    };

    let subject = state.subject(tenant, &team, envelope.channel_id.as_str());

    let value =
        to_value(&envelope).map_err(|err| (StatusCode::SERVICE_UNAVAILABLE, err.to_string()))?;
    state
        .bus
        .publish_value(&subject, value.clone())
        .await
        .map_err(|err| match err {
            BusError::Publish(e) => {
                tracing::error!(%subject, error = %e, "failed to publish ingress envelope");
                (StatusCode::SERVICE_UNAVAILABLE, e.to_string())
            }
        })?;

    let selected_cfg = state
        .config
        .worker_routing
        .clone()
        .or_else(|| state.config.worker_routes.values().next().cloned())
        .or_else(|| state.worker_default.clone());

    if let (Some(cfg), Some(egress_subject)) = (selected_cfg, state.worker_egress_subject.as_ref())
    {
        let worker = state
            .workers
            .get(cfg.worker_id.as_str())
            .or_else(|| state.workers.values().next());

        if let Some(worker) = worker {
            let correlation = Some(msg_id.clone());
            let worker_payload = envelope.payload.clone();
            let channel_clone = envelope.clone();
            let worker_result = forward_to_worker(
                worker.as_ref(),
                &channel_clone,
                worker_payload,
                &cfg,
                correlation,
            )
            .await;

            match worker_result {
                Ok(outbound) => {
                    let platform = Platform::from_str(envelope.channel_id.as_str())
                        .unwrap_or(Platform::WebChat);
                    for out in outbound {
                        let out_msg = outbound_to_out_message(
                            out,
                            platform.clone(),
                            envelope.payload["thread_id"].as_str().map(str::to_string),
                        );
                        let out_value = to_value(&out_msg)
                            .map_err(|err| (StatusCode::SERVICE_UNAVAILABLE, err.to_string()))?;
                        if let Err(err) = state.bus.publish_value(egress_subject, out_value).await {
                            tracing::error!(
                                subject = %egress_subject,
                                error = %err,
                                "failed to publish worker response to egress"
                            );
                        }
                    }
                }
                Err(err) => {
                    tracing::error!(
                        error = %err,
                        worker_id = %cfg.worker_id,
                        "failed to forward to repo worker"
                    );
                }
            }
        }
    }

    let _ = counter!(
        "messaging_ingress_total",
        "tenant" => tenant.to_string(),
        "platform" => envelope.channel_id.clone(),
        "adapter" => adapter.map(|a| a.name.clone()).unwrap_or_else(|| "legacy".into())
    );

    tracing::info_span!(
        "ingress_request",
        tenant = %tenant,
        platform = %envelope.channel_id,
        adapter = %adapter.as_ref().map(|a| a.name.as_str()).unwrap_or("legacy")
    )
    .in_scope(|| tracing::trace!("ingress request dispatched"));

    Ok(subject)
}

fn resolve_ingress_target(
    channel: &str,
    registry: &AdapterRegistry,
) -> Result<(Platform, Option<AdapterDescriptor>), (StatusCode, String)> {
    if let Some(adapter) = registry.get(channel) {
        if !adapter.allows_ingress() {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("adapter `{channel}` does not support ingress"),
            ));
        }
        let platform = infer_platform_from_adapter_name(&adapter.name)
            .or_else(|| Platform::from_str(channel).ok())
            .ok_or((
                StatusCode::BAD_REQUEST,
                format!("adapter `{channel}` does not map to a known platform"),
            ))?;
        return Ok((platform, Some(adapter.clone())));
    }
    if !registry.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            format!(
                "adapter `{channel}` not found; available: {}",
                registry.names().join(", ")
            ),
        ));
    }
    let platform = Platform::from_str(channel).map_err(|err| {
        (
            StatusCode::BAD_REQUEST,
            format!("invalid platform name: {err}"),
        )
    })?;
    Ok((platform, None))
}

#[cfg(test)]
mod tests_ingress_resolution {
    use super::*;
    use gsm_core::MessagingAdapterKind;

    fn adapter(name: &str, kind: MessagingAdapterKind) -> AdapterDescriptor {
        AdapterDescriptor {
            pack_id: "test-pack".into(),
            pack_version: "1.0.0".into(),
            name: name.into(),
            kind,
            component: "comp@1.0.0".into(),
            default_flow: None,
            custom_flow: None,
            capabilities: None,
            source: None,
        }
    }

    #[test]
    fn resolves_adapter_with_ingress_support() {
        let mut registry = AdapterRegistry::default();
        registry
            .register(adapter("slack-main", MessagingAdapterKind::IngressEgress))
            .unwrap();
        let (platform, resolved) =
            resolve_ingress_target("slack-main", &registry).expect("should resolve");
        assert_eq!(platform, Platform::Slack);
        assert_eq!(resolved.unwrap().name, "slack-main");
    }

    #[test]
    fn rejects_adapter_without_ingress() {
        let mut registry = AdapterRegistry::default();
        registry
            .register(adapter("slack-main", MessagingAdapterKind::Egress))
            .unwrap();
        let err = resolve_ingress_target("slack-main", &registry).unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        assert!(err.1.contains("does not support ingress"));
    }

    #[test]
    fn errors_when_adapter_missing_but_registry_present() {
        let mut registry = AdapterRegistry::default();
        registry
            .register(adapter("slack-main", MessagingAdapterKind::IngressEgress))
            .unwrap();
        let err = resolve_ingress_target("unknown", &registry).unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        assert!(err.1.contains("available: slack-main"));
    }

    #[test]
    fn falls_back_to_platform_when_no_registry() {
        let registry = AdapterRegistry::default();
        let (platform, resolved) =
            resolve_ingress_target("slack", &registry).expect("platform should parse");
        assert_eq!(platform, Platform::Slack);
        assert!(resolved.is_none());
    }
}

fn sanitize_team(team: Option<&str>, default: &str) -> String {
    match team {
        Some(value) if !value.trim().is_empty() => value.trim().to_string(),
        _ => default.to_string(),
    }
}

fn headers_to_json(headers: &HeaderMap) -> Value {
    let mut map = serde_json::Map::new();
    for (key, value) in headers.iter() {
        if let Ok(val) = value.to_str() {
            map.insert(key.as_str().to_string(), Value::String(val.to_string()));
        }
    }
    Value::Object(map)
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
