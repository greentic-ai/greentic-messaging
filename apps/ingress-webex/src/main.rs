use anyhow::Result;
use async_nats::Client as NatsClient;
use async_trait::async_trait;
use axum::{
    Router,
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header::HeaderName},
    response::IntoResponse,
    routing::{get, post},
};
#[cfg(not(test))]
use gsm_core::DefaultResolver;
use gsm_core::platforms::webex::{creds::WebexCredentials, provision::ensure_webhooks};
use gsm_core::telemetry::{install as init_telemetry, set_current_tenant_ctx};
use gsm_core::{
    NodeResult, Platform, Provider, ProviderKey, ProviderRegistry, TeamId, TenantCtx, UserId,
    in_subject, make_tenant_ctx,
};
use gsm_idempotency::{IdKey, IdempotencyGuard};
use gsm_ingress_common::{
    SharedSessionStore, SignatureAlgorithm, attach_session_id, init_guard, init_session_store,
    record_idempotency_hit, record_ingress, signature_header_from_env, start_ingress_span,
};
use serde_json::Value;
use std::{net::SocketAddr, str::FromStr, sync::Arc};
use tracing::{error, info, warn};

mod normalise;
mod verify;

use normalise::normalise_webhook;
use verify::verify_signature;

type SharedPublisher = Arc<dyn Publisher>;

#[cfg(test)]
mod test_support {
    use super::*;
    use gsm_core::{NodeError, SecretPath, SecretsResolver};
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[derive(Default)]
    pub(super) struct InMemorySecrets {
        store: Mutex<HashMap<String, Value>>,
    }

    #[async_trait]
    impl SecretsResolver for InMemorySecrets {
        async fn get_json<T>(&self, path: &SecretPath, _ctx: &TenantCtx) -> NodeResult<Option<T>>
        where
            T: serde::de::DeserializeOwned + Send,
        {
            let value = self.store.lock().unwrap().get(path.as_str()).cloned();
            if let Some(json) = value {
                Ok(Some(serde_json::from_value(json).map_err(|err| {
                    NodeError::new("decode", format!("failed to decode secret: {err}"))
                })?))
            } else {
                Ok(None)
            }
        }

        async fn put_json<T>(
            &self,
            path: &SecretPath,
            _ctx: &TenantCtx,
            value: &T,
        ) -> NodeResult<()>
        where
            T: serde::Serialize + Sync + Send,
        {
            let json = serde_json::to_value(value).map_err(|err| {
                NodeError::new("encode", format!("failed to encode secret: {err}"))
            })?;
            self.store
                .lock()
                .unwrap()
                .insert(path.as_str().to_string(), json);
            Ok(())
        }
    }
}

#[cfg(test)]
type Resolver = test_support::InMemorySecrets;
#[cfg(not(test))]
type Resolver = DefaultResolver;

#[async_trait]
pub trait Publisher: Send + Sync {
    async fn publish(&self, subject: &str, payload: Vec<u8>) -> anyhow::Result<()>;
}

#[derive(Clone)]
struct NatsPublisher {
    client: NatsClient,
}

#[async_trait]
impl Publisher for NatsPublisher {
    async fn publish(&self, subject: &str, payload: Vec<u8>) -> anyhow::Result<()> {
        self.client
            .publish(subject.to_string(), payload.into())
            .await?;
        Ok(())
    }
}

#[derive(Clone)]
struct AppState {
    registry: Arc<ProviderRegistry<WebexProvider>>,
    resolver: Arc<Resolver>,
    http_client: Arc<reqwest::Client>,
    webhook_base: String,
    api_base: String,
    signature_header: HeaderName,
    signature_algorithm: SignatureAlgorithm,
    guard: IdempotencyGuard,
    publisher: SharedPublisher,
    sessions: SharedSessionStore,
}

#[derive(Clone)]
struct WebexProvider {
    creds: WebexCredentials,
}

impl Provider for WebexProvider {}

fn router(state: AppState) -> Router {
    Router::new()
        .route("/ingress/webex/{tenant}", post(handle_webex_default_team))
        .route("/ingress/webex/{tenant}/{team}", post(handle_webex))
        .route("/healthz", get(healthz))
        .with_state(state)
}

#[tokio::main]
async fn main() -> Result<()> {
    init_telemetry("greentic-messaging")?;
    let nats_url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
    let header_name = signature_header_from_env();
    let signature_header =
        HeaderName::from_str(&header_name).expect("invalid header name in WEBEX_SIG_HEADER");
    let signature_algorithm = SignatureAlgorithm::from_env();

    let webhook_base =
        std::env::var("WEBEX_WEBHOOK_BASE").unwrap_or_else(|_| "http://localhost:8088".into());
    let api_base =
        std::env::var("WEBEX_API_BASE").unwrap_or_else(|_| "https://webexapis.com/v1".into());

    #[cfg(test)]
    let resolver = Arc::new(Resolver::default());
    #[cfg(not(test))]
    let resolver = Arc::new(Resolver::new().await?);
    let registry = Arc::new(ProviderRegistry::new());
    let http_client = Arc::new(reqwest::Client::new());

    let nats = async_nats::connect(nats_url).await?;
    let guard = init_guard(&nats).await?;
    let publisher: SharedPublisher = Arc::new(NatsPublisher { client: nats });
    let sessions = init_session_store().await?;

    let state = AppState {
        registry,
        resolver,
        http_client,
        webhook_base,
        api_base,
        signature_header,
        signature_algorithm,
        guard,
        publisher,
        sessions,
    };

    let addr: SocketAddr = std::env::var("BIND")
        .unwrap_or_else(|_| "0.0.0.0:8088".into())
        .parse()
        .expect("invalid BIND address");

    info!("ingress-webex listening on {addr}");
    let app = router(state);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app.into_make_service()).await?;
    Ok(())
}

async fn healthz() -> impl IntoResponse {
    StatusCode::NO_CONTENT
}

#[derive(Debug, Clone)]
struct WebexPath {
    tenant: String,
    team: Option<String>,
}

fn normalize_team(team: Option<&str>) -> Option<String> {
    match team {
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("default") {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        None => None,
    }
}

async fn handle_webex(
    State(state): State<AppState>,
    Path((tenant, team)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let path = WebexPath {
        tenant,
        team: Some(team),
    };
    match process_webhook(state, path, headers, body).await {
        Ok(status) => status,
        Err(status) => status,
    }
}

async fn handle_webex_default_team(
    State(state): State<AppState>,
    Path(tenant): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let path = WebexPath { tenant, team: None };
    match process_webhook(state, path, headers, body).await {
        Ok(status) => status,
        Err(status) => status,
    }
}

async fn process_webhook(
    state: AppState,
    path: WebexPath,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, StatusCode> {
    let ctx_base = make_tenant_ctx(
        path.tenant.clone(),
        normalize_team(path.team.as_deref()),
        None,
    );
    let provider = resolve_provider(&state, &ctx_base, &path)
        .await
        .map_err(|err| {
            error!(error = %err, "webex provider resolution failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let signature_value = headers
        .get(&state.signature_header)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| {
            warn!("missing webex signature header");
            StatusCode::UNAUTHORIZED
        })?;

    let verified = verify_signature(
        &provider.creds.webhook_secret,
        signature_value,
        &body,
        state.signature_algorithm,
    )
    .map_err(|err| {
        error!(error = %err, "failed to verify webex signature");
        StatusCode::UNAUTHORIZED
    })?;

    if !verified {
        warn!("invalid webex signature");
        return Err(StatusCode::UNAUTHORIZED);
    }

    let raw: Value = serde_json::from_slice(&body).map_err(|err| {
        error!(error = %err, "failed to decode webex webhook");
        StatusCode::BAD_REQUEST
    })?;

    let envelope = normalise_webhook(&ctx_base, &raw).map_err(|err| {
        error!(error = %err, "failed to normalise webex payload");
        StatusCode::BAD_REQUEST
    })?;

    let mut ctx = ctx_base.clone();
    ctx.team = Some(TeamId(envelope.chat_id.clone()));
    ctx.user = Some(UserId(envelope.user_id.clone()));

    let span = start_ingress_span(&envelope);
    let _guard = span.enter();

    let key = IdKey {
        tenant: envelope.tenant.clone(),
        platform: envelope.platform.as_str().to_string(),
        msg_id: envelope.msg_id.clone(),
    };

    match state.guard.should_process(&key).await {
        Ok(true) => {}
        Ok(false) => {
            record_idempotency_hit(&key.tenant);
            info!(
                tenant = %key.tenant,
                platform = %key.platform,
                msg_id = %key.msg_id,
                "duplicate webex event dropped"
            );
            return Ok(StatusCode::OK);
        }
        Err(err) => {
            error!(error = %err, tenant = %key.tenant, "idempotency check failed");
        }
    }

    let mut invocation = envelope.clone().into_invocation().map_err(|err| {
        error!(error = %err, "failed to build invocation envelope");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    invocation.ctx = ctx.clone();
    attach_session_id(&state.sessions, &ctx, &envelope, &mut invocation).await;

    let subject = in_subject(
        ctx.tenant.as_str(),
        envelope.platform.as_str(),
        &envelope.chat_id,
    );
    let payload = serde_json::to_vec(&invocation).map_err(|err| {
        error!(error = %err, "failed to serialise invocation");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    set_current_tenant_ctx(invocation.ctx.clone());

    state
        .publisher
        .publish(&subject, payload)
        .await
        .map_err(|err| {
            error!(error = %err, subject = %subject, "failed to publish to nats");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    record_ingress(&envelope);
    info!(chat_id = %envelope.chat_id, "webex message published");

    Ok(StatusCode::OK)
}

async fn resolve_provider(
    state: &AppState,
    ctx: &TenantCtx,
    path: &WebexPath,
) -> NodeResult<Arc<WebexProvider>> {
    let key = provider_key(ctx);
    if let Some(existing) = state.registry.get(&key) {
        return Ok(existing);
    }

    let mut target_url = format!(
        "{}/ingress/webex/{}",
        state.webhook_base.trim_end_matches('/'),
        path.tenant
    );
    if let Some(team) = path
        .team
        .as_ref()
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
    {
        target_url.push('/');
        target_url.push_str(team);
    }

    let creds = ensure_webhooks(
        &state.http_client,
        ctx,
        &target_url,
        &state.api_base,
        state.resolver.as_ref(),
    )
    .await?;

    let provider = Arc::new(WebexProvider { creds });
    state.registry.put(key, provider.clone());
    Ok(provider)
}

fn provider_key(ctx: &TenantCtx) -> ProviderKey {
    ProviderKey {
        platform: Platform::Webex,
        env: ctx.env.clone(),
        tenant: ctx.tenant.clone(),
        team: ctx.team.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use gsm_core::{
        InvocationEnvelope, MessageEnvelope, Platform, SecretsResolver, make_tenant_ctx,
        webex_credentials,
    };
    use gsm_idempotency::{InMemoryIdemStore, SharedIdemStore};
    use std::sync::Mutex;
    use tower::ServiceExt;
    type EventLog = Arc<Mutex<Vec<(String, Vec<u8>)>>>;

    #[derive(Clone, Default)]
    struct MockPublisher {
        events: EventLog,
    }

    #[async_trait]
    impl Publisher for MockPublisher {
        async fn publish(&self, subject: &str, payload: Vec<u8>) -> anyhow::Result<()> {
            self.events
                .lock()
                .unwrap()
                .push((subject.to_string(), payload));
            Ok(())
        }
    }

    async fn build_state() -> (AppState, Arc<MockPublisher>) {
        unsafe {
            std::env::set_var("GREENTIC_ENV", "test");
        }
        let signature_header = HeaderName::from_static("x-webex-signature");
        let signature_algorithm = SignatureAlgorithm::Sha1;
        let store: SharedIdemStore = Arc::new(InMemoryIdemStore::new());
        let guard = IdempotencyGuard::new(store, 1);
        let mock = Arc::new(MockPublisher::default());
        let publisher: SharedPublisher = mock.clone();
        let sessions = init_session_store().await.expect("session store");

        let resolver = Arc::new(super::Resolver::default());
        let registry = Arc::new(ProviderRegistry::new());
        let http_client = Arc::new(reqwest::Client::new());

        let ctx = make_tenant_ctx("acme".into(), None, None);
        let creds = WebexCredentials {
            bot_token: "TOKEN".into(),
            webhook_secret: "top-secret".into(),
            webhooks: Vec::new(),
        };
        resolver
            .as_ref()
            .put_json(&webex_credentials(&ctx), &ctx, &creds)
            .await
            .unwrap();

        (
            AppState {
                registry,
                resolver,
                http_client,
                webhook_base: "http://localhost:8088".into(),
                api_base: "mock://webex".into(),
                signature_header,
                signature_algorithm,
                guard,
                publisher,
                sessions,
            },
            mock,
        )
    }

    fn sign(secret: &str, body: &[u8]) -> String {
        let sig = super::verify::compute_signature(secret, body, SignatureAlgorithm::Sha1)
            .expect("signature");
        format!("sha1={}", hex::encode(sig))
    }

    fn build_request(path: &WebexPath, body: &str, signature: &str) -> Request<Body> {
        let uri = match path
            .team
            .as_ref()
            .map(|t| t.trim())
            .filter(|t| !t.is_empty())
        {
            Some(team) => format!("/ingress/webex/{}/{}", path.tenant, team),
            None => format!("/ingress/webex/{}", path.tenant),
        };
        Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json")
            .header("x-webex-signature", signature)
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    #[tokio::test]
    async fn publishes_and_dedupes() {
        let body = r#"{
            "resource": "messages",
            "event": "created",
            "data": {
                "id": "mid-001",
                "roomId": "room-9",
                "personId": "person-7",
                "created": "2024-01-01T00:00:00Z",
                "text": "hi"
            }
        }"#;

        let (state, publisher) = build_state().await;
        let app = router(state.clone());
        let path = WebexPath {
            tenant: "acme".into(),
            team: Some("default".into()),
        };

        let signature = sign("top-secret", body.as_bytes());
        let req = build_request(&path, body, &signature);
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        let req2 = build_request(&path, body, &signature);
        let res2 = app.oneshot(req2).await.unwrap();
        assert_eq!(res2.status(), StatusCode::OK);

        let stored = publisher.events.lock().unwrap().clone();
        assert_eq!(stored.len(), 1, "duplicate payload should not republish");

        let (subject, payload) = &stored[0];
        assert_eq!(subject, "greentic.msg.in.acme.webex.room-9");
        let invocation: InvocationEnvelope = serde_json::from_slice(payload).unwrap();
        assert_eq!(invocation.ctx.tenant.as_str(), "acme");
        assert_eq!(
            invocation.ctx.team.as_ref().map(|t| t.as_str()),
            Some("room-9")
        );
        assert_eq!(
            invocation.ctx.user.as_ref().map(|u| u.as_str()),
            Some("person-7")
        );
        let env = MessageEnvelope::try_from(invocation).expect("message envelope");
        assert_eq!(env.platform, Platform::Webex);
        assert_eq!(env.msg_id, "mid-001");
    }

    #[tokio::test]
    async fn publishes_without_team_segment() {
        let body = r#"{
            "resource": "messages",
            "event": "created",
            "data": {
                "id": "mid-001",
                "roomId": "room-9",
                "personId": "person-7",
                "created": "2024-01-01T00:00:00Z",
                "text": "hi"
            }
        }"#;

        let (state, publisher) = build_state().await;
        let app = router(state);
        let path = WebexPath {
            tenant: "acme".into(),
            team: None,
        };

        let signature = sign("top-secret", body.as_bytes());
        let req = build_request(&path, body, &signature);
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        let stored = publisher.events.lock().unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].0, "greentic.msg.in.acme.webex.room-9");
    }
}
