//! Standalone Direct Line server implementation.
//!
//! This module exposes a Direct Line–compatible surface area that aligns with
//! Microsoft Bot Framework expectations while keeping all state within the
//! Greentic stack. Tokens are minted locally and conversations stream through
//! the in-memory [`ConversationStore`].

use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::Context as _;
use axum::{
    Json, Router,
    extract::{
        ConnectInfo, Extension, FromRequestParts, Path, Query, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::{HeaderMap, StatusCode, request::Parts},
    response::IntoResponse,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use tracing::{debug, error, warn};
use uuid::Uuid;

use super::{
    WebChatProvider,
    auth::{self as jwt, Claims, TenantClaims},
    bus::{NoopBus, SharedBus},
    conversation::{
        Activity, ChannelAccount, ConversationAccount, SharedConversationStore, StoredActivity,
        memory_store,
    },
    ingress,
    session::{MemorySessionStore, SharedSessionStore, WebchatSession},
};
use greentic_types::{EnvId, TeamId, TenantCtx, TenantId};

const TOKEN_TTL_SECONDS: u64 = 1_800;
const RATE_LIMIT_CAPACITY: usize = 5;
const RATE_LIMIT_WINDOW: Duration = Duration::from_secs(60);

struct RemoteIp(Option<IpAddr>);

impl<S> FromRequestParts<S> for RemoteIp
where
    S: Send + Sync,
{
    type Rejection = std::convert::Infallible;

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl std::future::Future<Output = Result<Self, Self::Rejection>> + Send {
        let ip = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|ConnectInfo(addr)| addr.ip());
        std::future::ready(Ok(RemoteIp(ip)))
    }
}

/// Shared state for the standalone Direct Line server.
#[derive(Clone)]
pub struct StandaloneState {
    pub provider: WebChatProvider,
    pub conversations: SharedConversationStore,
    pub sessions: SharedSessionStore,
    pub bus: SharedBus,
    rate_limiter: Arc<IpRateLimiter>,
}

impl StandaloneState {
    /// Builds a new state wired to the provided conversation store.
    pub async fn with_store(
        provider: WebChatProvider,
        conversations: SharedConversationStore,
        sessions: SharedSessionStore,
        bus: SharedBus,
    ) -> anyhow::Result<Self> {
        let signing_keys = provider
            .signing_keys()
            .await
            .context("failed to resolve Direct Line signing key")?;
        if let Err(err) = jwt::install_keys(signing_keys) {
            debug!("jwt keys already installed: {err}");
        }
        Ok(Self {
            provider,
            conversations,
            sessions,
            bus,
            rate_limiter: Arc::new(IpRateLimiter::new(RATE_LIMIT_CAPACITY, RATE_LIMIT_WINDOW)),
        })
    }

    /// Builds a new state with the default in-memory conversation store.
    pub async fn new(provider: WebChatProvider) -> anyhow::Result<Self> {
        Self::with_store(
            provider,
            memory_store(),
            Arc::new(MemorySessionStore::default()),
            Arc::new(NoopBus),
        )
        .await
    }
}

fn router_blueprint() -> Router {
    Router::new()
        .route(
            "/v3/directline/tokens/generate",
            post(generate_token_handler),
        )
        .route(
            "/v3/directline/conversations",
            post(start_conversation_handler),
        )
        .route(
            "/v3/directline/conversations/{id}/activities",
            get(list_activities_handler).post(post_activity_handler),
        )
        .route(
            "/v3/directline/conversations/{id}/stream",
            get(conversation_stream_handler),
        )
        .route(
            "/webchat/admin/{env}/{tenant}/post-activity",
            post(admin_post_activity_handler),
        )
}

/// Builds the Axum router that serves `/v3/directline` endpoints.
pub fn router(state: Arc<StandaloneState>) -> Router {
    router_blueprint().layer(axum::Extension(state))
}

#[allow(dead_code)]
const fn assert_state_bounds<T: Send + Sync + Clone>() {}
const _: () = {
    assert_state_bounds::<StandaloneState>();
};

#[doc(hidden)]
#[derive(Debug, Deserialize)]
pub struct TenantQuery {
    env: String,
    tenant: String,
    #[serde(default)]
    team: Option<String>,
}

#[doc(hidden)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateTokenRequest {
    #[serde(default)]
    user: Option<UserDescriptor>,
    #[allow(dead_code)]
    #[serde(default)]
    trusted_origins: Option<Vec<String>>,
}

#[doc(hidden)]
#[derive(Debug, Deserialize)]
pub struct UserDescriptor {
    #[serde(default)]
    id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct TokenResponse {
    token: String,
    expires_in: u64,
}

async fn generate_token_handler(
    Extension(state): Extension<Arc<StandaloneState>>,
    remote: RemoteIp,
    Query(query): Query<TenantQuery>,
    Json(body): Json<GenerateTokenRequest>,
) -> Result<Json<TokenResponse>, StatusCode> {
    let ip = remote.0.unwrap_or_else(|| IpAddr::from([127, 0, 0, 1]));
    if !state.rate_limiter.check(ip) {
        return Err(StatusCode::TOO_MANY_REQUESTS);
    }

    let tenant_ctx = tenant_ctx_from_query(&query)?;
    let subject = body
        .user
        .and_then(|user| user.id)
        .filter(|id| !id.trim().is_empty())
        .unwrap_or_else(|| format!("user:{}", Uuid::new_v4()));

    let claims = Claims::new(
        subject,
        tenant_claims_from_ctx(&tenant_ctx),
        jwt::ttl(TOKEN_TTL_SECONDS),
    );
    let token = jwt::sign(&claims).map_err(|err| map_error("sign token", err))?;
    Ok(Json(TokenResponse {
        token,
        expires_in: TOKEN_TTL_SECONDS,
    }))
}

#[doc(hidden)]
#[derive(Debug, Deserialize)]
pub struct ConversationPath {
    id: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConversationResponse {
    conversation_id: String,
    token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_url: Option<String>,
}

async fn start_conversation_handler(
    Extension(state): Extension<Arc<StandaloneState>>,
    headers: HeaderMap,
) -> Result<Json<ConversationResponse>, StatusCode> {
    let token = extract_bearer(&headers)?;
    let claims = jwt::verify(token).map_err(|_| StatusCode::UNAUTHORIZED)?;
    if claims.conv.is_some() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let tenant_ctx = tenant_ctx_from_claims(&claims)?;
    let conversation_id = Uuid::new_v4().to_string();
    state
        .conversations
        .create(&conversation_id, tenant_ctx.clone())
        .await
        .map_err(map_store_error)?;

    let conversation_token = Claims::new(
        claims.sub.clone(),
        claims.ctx.clone(),
        jwt::ttl(TOKEN_TTL_SECONDS),
    )
    .with_conversation(conversation_id.clone());
    let token = jwt::sign(&conversation_token).map_err(|err| map_error("sign token", err))?;

    if let Err(err) = state
        .sessions
        .upsert(WebchatSession::new(
            conversation_id.clone(),
            tenant_ctx.clone(),
            token.clone(),
        ))
        .await
    {
        error!(error = %err, "failed to persist webchat session");
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }

    let stream_url = stream_url_from_headers(&state, &headers, &conversation_id, &token);
    Ok(Json(ConversationResponse {
        conversation_id,
        token,
        stream_url,
    }))
}

#[doc(hidden)]
#[derive(Debug, Deserialize)]
pub struct ActivitiesQuery {
    #[serde(default)]
    watermark: Option<String>,
}

#[derive(Debug, Serialize)]
struct ActivitiesResponse {
    activities: Vec<Activity>,
    watermark: String,
}

async fn list_activities_handler(
    Extension(state): Extension<Arc<StandaloneState>>,
    Path(path): Path<ConversationPath>,
    headers: HeaderMap,
    Query(query): Query<ActivitiesQuery>,
) -> Result<Json<ActivitiesResponse>, StatusCode> {
    let _claims = validate_conversation_token(state.as_ref(), &path.id, &headers).await?;
    let watermark = query
        .watermark
        .as_deref()
        .and_then(|value| {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        })
        .map(parse_watermark)
        .transpose()?;
    let page = state
        .conversations
        .activities(&path.id, watermark)
        .await
        .map_err(map_store_error)?;
    let activities: Vec<Activity> = page
        .activities
        .into_iter()
        .map(|entry| entry.activity)
        .collect();
    Ok(Json(ActivitiesResponse {
        activities,
        watermark: page.watermark.to_string(),
    }))
}

#[derive(Debug, Serialize)]
struct ActivityAck {
    id: String,
}

async fn post_activity_handler(
    Extension(state): Extension<Arc<StandaloneState>>,
    Path(path): Path<ConversationPath>,
    headers: HeaderMap,
    Json(payload): Json<serde_json::Value>,
) -> Result<impl IntoResponse, StatusCode> {
    let claims = validate_conversation_token(state.as_ref(), &path.id, &headers).await?;
    let tenant_ctx = state
        .conversations
        .tenant_ctx(&path.id)
        .await
        .map_err(map_store_error)?;

    let mut bus_activity = payload.clone();
    apply_user_json_defaults(&mut bus_activity, &path.id, &claims.sub);

    let mut activity: Activity =
        serde_json::from_value(payload).map_err(|_| StatusCode::BAD_REQUEST)?;
    apply_user_defaults(&mut activity, &path.id, &claims.sub);

    let ingress = ingress::Ingress::new(state.bus.clone(), state.sessions.clone());
    if let Err(err) = ingress
        .publish_incoming(&bus_activity, &tenant_ctx, &path.id)
        .await
    {
        warn!(error = %err, "failed to publish incoming activity");
    }

    let stored = state
        .conversations
        .append(&path.id, activity)
        .await
        .map_err(map_store_error)?;

    if let Err(err) = state
        .sessions
        .update_watermark(&path.id, Some((stored.watermark + 1).to_string()))
        .await
    {
        warn!(error = %err, "failed to update watermark");
    }
    Ok((
        StatusCode::CREATED,
        Json(ActivityAck {
            id: stored.activity.id,
        }),
    ))
}

#[derive(Debug, Deserialize)]
pub struct AdminPath {
    pub env: String,
    pub tenant: String,
}

#[derive(Debug, Deserialize)]
pub struct AdminPostActivityRequest {
    #[serde(default)]
    pub team: Option<String>,
    #[serde(rename = "conversation_id", default)]
    pub conversation_id: Option<String>,
    pub activity: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct AdminPostActivityResponse {
    pub posted: usize,
    pub skipped: usize,
}

async fn admin_post_activity_handler(
    Extension(state): Extension<Arc<StandaloneState>>,
    Path(path): Path<AdminPath>,
    Json(body): Json<AdminPostActivityRequest>,
) -> Result<Json<AdminPostActivityResponse>, StatusCode> {
    let AdminPostActivityRequest {
        team,
        conversation_id,
        activity,
    } = body;

    if !activity.is_object() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let base_activity: Activity =
        serde_json::from_value(activity).map_err(|_| StatusCode::BAD_REQUEST)?;
    let team_filter = team.as_deref();

    if let Some(conversation_id) = conversation_id {
        let session = state
            .sessions
            .get(&conversation_id)
            .await
            .map_err(|err| map_error("load session", err))?
            .ok_or(StatusCode::NOT_FOUND)?;

        if !session
            .tenant_ctx
            .env
            .as_ref()
            .eq_ignore_ascii_case(&path.env)
            || !session
                .tenant_ctx
                .tenant
                .as_ref()
                .eq_ignore_ascii_case(&path.tenant)
        {
            return Err(StatusCode::NOT_FOUND);
        }

        if let Some(team) = team_filter
            && !session
                .tenant_ctx
                .team
                .as_ref()
                .map(|value| value.as_ref().eq_ignore_ascii_case(team))
                .unwrap_or(false)
        {
            return Err(StatusCode::NOT_FOUND);
        }

        if !session.proactive_ok {
            return Err(StatusCode::BAD_REQUEST);
        }

        append_bot_activity(state.as_ref(), &conversation_id, &base_activity).await?;

        return Ok(Json(AdminPostActivityResponse {
            posted: 1,
            skipped: 0,
        }));
    }

    let sessions = state
        .sessions
        .list_by_tenant(&path.env, &path.tenant, team_filter)
        .await
        .map_err(|err| map_error("list sessions", err))?;

    if sessions.is_empty() {
        return Err(StatusCode::NOT_FOUND);
    }

    let mut posted = 0usize;
    let mut skipped = 0usize;
    for session in sessions {
        if !session.proactive_ok {
            skipped += 1;
            continue;
        }

        match append_bot_activity(state.as_ref(), &session.conversation_id, &base_activity).await {
            Ok(()) => posted += 1,
            Err(StatusCode::NOT_FOUND) => {
                skipped += 1;
                warn!(
                    conversation = %session.conversation_id,
                    "conversation not found while appending activity"
                );
            }
            Err(code) => return Err(code),
        }
    }

    if posted == 0 {
        return Err(StatusCode::NOT_FOUND);
    }

    Ok(Json(AdminPostActivityResponse { posted, skipped }))
}

#[derive(Debug, Deserialize)]
struct StreamQuery {
    t: String,
    #[serde(default)]
    watermark: Option<String>,
}

async fn conversation_stream_handler(
    Extension(state): Extension<Arc<StandaloneState>>,
    Path(path): Path<ConversationPath>,
    Query(query): Query<StreamQuery>,
    ws: WebSocketUpgrade,
) -> Result<impl IntoResponse, StatusCode> {
    let claims = jwt::verify(&query.t).map_err(|_| StatusCode::UNAUTHORIZED)?;
    ensure_conversation_access(state.as_ref(), &path.id, &claims).await?;
    let watermark = query
        .watermark
        .as_deref()
        .map(parse_watermark)
        .transpose()?;
    Ok(ws.on_upgrade(move |socket| async move {
        if let Err(err) = run_websocket(socket, state, path.id, watermark).await {
            warn!("websocket closed with error: {err:?}");
        }
    }))
}

async fn run_websocket(
    mut socket: WebSocket,
    state: Arc<StandaloneState>,
    conversation_id: String,
    watermark: Option<u64>,
) -> anyhow::Result<()> {
    let page = state
        .conversations
        .activities(&conversation_id, watermark)
        .await
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    let initial: Vec<StoredActivity> = page.activities.iter().cloned().collect();
    if !initial.is_empty() {
        send_envelope(&mut socket, &initial, page.watermark).await?;
    }
    let mut consecutive_failures: u32 = 0;
    const SEND_FAILURE_THRESHOLD: u32 = 5;
    let mut current_watermark = page.watermark;
    let mut subscriber = state
        .conversations
        .subscribe(&conversation_id)
        .await
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;

    loop {
        tokio::select! {
            message = socket.recv() => {
                match message {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => continue,
                    Some(Err(err)) => {
                        warn!("websocket recv error: {err}");
                        break;
                    }
                }
            }
            received = subscriber.recv() => {
                match received {
                    Ok(activity) => {
                        if let Err(err) = send_envelope(&mut socket, std::slice::from_ref(&activity), activity.watermark + 1).await {
                            consecutive_failures = consecutive_failures.saturating_add(1);
                            warn!(error = ?err, consecutive_failures, "websocket send error");
                            if consecutive_failures >= SEND_FAILURE_THRESHOLD {
                                warn!("terminating websocket due to repeated send failures");
                                break;
                            }
                            continue;
                        } else {
                            consecutive_failures = 0;
                        }
                        current_watermark = activity.watermark + 1;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        let page = state.conversations.activities(&conversation_id, Some(current_watermark))
                            .await
                            .map_err(|err| anyhow::anyhow!(err.to_string()))?;
                        let resend: Vec<StoredActivity> = page.activities.iter().cloned().collect();
                        if !resend.is_empty() {
                            if let Err(err) = send_envelope(&mut socket, &resend, page.watermark).await {
                                consecutive_failures = consecutive_failures.saturating_add(1);
                                warn!(error = ?err, consecutive_failures, "websocket resend error");
                                if consecutive_failures >= SEND_FAILURE_THRESHOLD {
                                    warn!("terminating websocket due to repeated resend failures");
                                    break;
                                }
                                continue;
                            } else {
                                consecutive_failures = 0;
                            }
                        }
                        current_watermark = page.watermark;
                    }
                }
            }
        }
    }
    Ok(())
}

#[doc(hidden)]
pub async fn test_run_websocket_public(
    socket: WebSocket,
    state: Arc<StandaloneState>,
    conversation_id: String,
    watermark: Option<u64>,
) -> anyhow::Result<()> {
    run_websocket(socket, state, conversation_id, watermark).await
}

async fn send_envelope(
    socket: &mut WebSocket,
    activities: &[StoredActivity],
    watermark: u64,
) -> anyhow::Result<()> {
    if activities.is_empty() {
        return Ok(());
    }
    let payload = envelope_payload(activities, watermark)?;
    socket.send(Message::Text(payload.into())).await?;
    Ok(())
}

fn envelope_payload(activities: &[StoredActivity], watermark: u64) -> anyhow::Result<String> {
    let acts: Vec<Activity> = activities
        .iter()
        .map(|entry| entry.activity.clone())
        .collect();
    Ok(serde_json::to_string(&serde_json::json!({
        "activities": acts,
        "watermark": watermark.to_string(),
    }))?)
}

fn tenant_ctx_from_query(query: &TenantQuery) -> Result<TenantCtx, StatusCode> {
    let env = query.env.trim();
    let tenant = query.tenant.trim();
    if env.is_empty() || tenant.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let mut ctx = TenantCtx::new(EnvId(env.to_string()), TenantId(tenant.to_string()));
    if let Some(team) = &query.team {
        let team = team.trim();
        if !team.is_empty() {
            ctx = ctx.with_team(Some(TeamId(team.to_string())));
        }
    }
    Ok(ctx)
}

fn tenant_ctx_from_claims(claims: &Claims) -> Result<TenantCtx, StatusCode> {
    let mut ctx = TenantCtx::new(
        EnvId(claims.ctx.env.clone()),
        TenantId(claims.ctx.tenant.clone()),
    );
    if let Some(team) = &claims.ctx.team {
        ctx = ctx.with_team(Some(TeamId(team.clone())));
    }
    Ok(ctx)
}

fn tenant_claims_from_ctx(ctx: &TenantCtx) -> TenantClaims {
    TenantClaims {
        env: ctx.env.as_ref().to_string(),
        tenant: ctx.tenant.as_ref().to_string(),
        team: ctx.team.as_ref().map(|team| team.as_ref().to_string()),
    }
}

fn extract_bearer(headers: &HeaderMap) -> Result<&str, StatusCode> {
    let value = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if let Some(rest) = value.strip_prefix("Bearer ") {
        Ok(rest.trim())
    } else if let Some(rest) = value.strip_prefix("bearer ") {
        Ok(rest.trim())
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}

fn parse_watermark(value: &str) -> Result<u64, StatusCode> {
    value.parse::<u64>().map_err(|_| StatusCode::BAD_REQUEST)
}

fn stream_url_from_headers(
    state: &StandaloneState,
    headers: &HeaderMap,
    conversation_id: &str,
    token: &str,
) -> Option<String> {
    if let Some(host) = headers
        .get(axum::http::header::HOST)
        .and_then(|value| value.to_str().ok())
    {
        let scheme = if state
            .provider
            .config()
            .direct_line_base()
            .starts_with("https://")
        {
            "wss"
        } else {
            "ws"
        };
        Some(format!(
            "{scheme}://{host}/v3/directline/conversations/{conversation_id}/stream?t={token}"
        ))
    } else {
        None
    }
}

async fn validate_conversation_token(
    state: &StandaloneState,
    conversation_id: &str,
    headers: &HeaderMap,
) -> Result<Claims, StatusCode> {
    let token = extract_bearer(headers)?;
    let claims = jwt::verify(token).map_err(|_| StatusCode::UNAUTHORIZED)?;
    ensure_conversation_access(state, conversation_id, &claims).await?;
    Ok(claims)
}

async fn ensure_conversation_access(
    state: &StandaloneState,
    conversation_id: &str,
    claims: &Claims,
) -> Result<(), StatusCode> {
    if !claims.has_conversation(conversation_id) {
        return Err(StatusCode::FORBIDDEN);
    }
    let claimed_ctx = tenant_ctx_from_claims(claims)?;
    let stored_ctx = state
        .conversations
        .tenant_ctx(conversation_id)
        .await
        .map_err(map_store_error)?;
    if stored_ctx != claimed_ctx {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(())
}

fn normalise_activity(activity: &mut Activity, conversation_id: &str, subject: &str) {
    if activity
        .from
        .as_ref()
        .map(|from| from.id.trim().is_empty())
        .unwrap_or(true)
    {
        activity.from = Some(ChannelAccount {
            id: subject.to_string(),
            name: None,
            role: Some("user".into()),
        });
    }
    if activity.conversation.is_none() {
        activity.conversation = Some(ConversationAccount {
            id: conversation_id.to_string(),
        });
    }
}

fn apply_bot_defaults(activity: &mut Activity, conversation_id: &str) {
    normalise_activity(activity, conversation_id, "bot");
    if let Some(from) = activity.from.as_mut() {
        if from.id.trim().is_empty() {
            from.id = "bot".into();
        }
        from.role = Some("bot".into());
    } else {
        activity.from = Some(ChannelAccount {
            id: "bot".into(),
            name: None,
            role: Some("bot".into()),
        });
    }
}

fn apply_user_json_defaults(
    activity: &mut serde_json::Value,
    conversation_id: &str,
    subject: &str,
) {
    let Some(obj) = activity.as_object_mut() else {
        return;
    };

    let from_entry = obj
        .entry("from".to_string())
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    if !from_entry.is_object() {
        *from_entry = serde_json::Value::Object(serde_json::Map::new());
    }
    if let Some(from_obj) = from_entry.as_object_mut() {
        let id_entry = from_obj
            .entry("id".to_string())
            .or_insert_with(|| serde_json::Value::String(subject.to_string()));
        if id_entry
            .as_str()
            .map(|value| value.trim().is_empty())
            .unwrap_or(true)
        {
            *id_entry = serde_json::Value::String(subject.to_string());
        }
        let role_entry = from_obj
            .entry("role".to_string())
            .or_insert_with(|| serde_json::Value::String("user".into()));
        if !role_entry
            .as_str()
            .map(|role| role.eq_ignore_ascii_case("user"))
            .unwrap_or(false)
        {
            *role_entry = serde_json::Value::String("user".into());
        }
    }

    let conversation_entry = obj.entry("conversation".to_string()).or_insert_with(|| {
        let mut conv = serde_json::Map::new();
        conv.insert(
            "id".to_string(),
            serde_json::Value::String(conversation_id.to_string()),
        );
        serde_json::Value::Object(conv)
    });
    if let Some(conv_obj) = conversation_entry.as_object_mut() {
        let id_entry = conv_obj
            .entry("id".to_string())
            .or_insert_with(|| serde_json::Value::String(conversation_id.to_string()));
        if id_entry
            .as_str()
            .map(|value| value.trim().is_empty())
            .unwrap_or(true)
        {
            *id_entry = serde_json::Value::String(conversation_id.to_string());
        }
    } else {
        let mut conv = serde_json::Map::new();
        conv.insert(
            "id".to_string(),
            serde_json::Value::String(conversation_id.to_string()),
        );
        *conversation_entry = serde_json::Value::Object(conv);
    }
}

fn apply_user_defaults(activity: &mut Activity, conversation_id: &str, subject: &str) {
    normalise_activity(activity, conversation_id, subject);
    if let Some(from) = activity.from.as_mut() {
        if from
            .role
            .as_deref()
            .map(|role| role.eq_ignore_ascii_case("user"))
            .unwrap_or(true)
        {
            from.role = Some("user".into());
        }
        if from.id.trim().is_empty() {
            from.id = subject.to_string();
        }
    } else {
        activity.from = Some(ChannelAccount {
            id: subject.to_string(),
            name: None,
            role: Some("user".into()),
        });
    }
}

async fn append_bot_activity(
    state: &StandaloneState,
    conversation_id: &str,
    base_activity: &Activity,
) -> Result<(), StatusCode> {
    let mut activity = base_activity.clone();
    apply_bot_defaults(&mut activity, conversation_id);
    let stored = state
        .conversations
        .append(conversation_id, activity)
        .await
        .map_err(map_store_error)?;

    if let Err(err) = state
        .sessions
        .update_watermark(conversation_id, Some((stored.watermark + 1).to_string()))
        .await
    {
        warn!(
            error = %err,
            conversation = %conversation_id,
            "failed to update watermark"
        );
    }

    Ok(())
}

fn map_store_error(err: super::conversation::StoreError) -> StatusCode {
    match err {
        super::conversation::StoreError::AlreadyExists(_) => StatusCode::CONFLICT,
        super::conversation::StoreError::NotFound(_) => StatusCode::NOT_FOUND,
        super::conversation::StoreError::QuotaExceeded(_) => StatusCode::TOO_MANY_REQUESTS,
        super::conversation::StoreError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

fn map_error(context: &str, err: anyhow::Error) -> StatusCode {
    error!("{context}: {err}");
    StatusCode::INTERNAL_SERVER_ERROR
}

struct IpRateLimiter {
    capacity: f64,
    refill_per_sec: f64,
    buckets: Mutex<HashMap<IpAddr, TokenBucket>>,
}

struct TokenBucket {
    tokens: f64,
    last: Instant,
}

impl IpRateLimiter {
    fn new(capacity: usize, window: Duration) -> Self {
        let capacity = capacity as f64;
        let refill_per_sec = if window.is_zero() {
            capacity
        } else {
            capacity / window.as_secs_f64()
        };
        Self {
            capacity,
            refill_per_sec,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    fn check(&self, ip: IpAddr) -> bool {
        let mut guard = self.buckets.lock().expect("rate limiter mutex poisoned");
        let entry = guard.entry(ip).or_insert(TokenBucket {
            tokens: self.capacity,
            last: Instant::now(),
        });
        let now = Instant::now();
        let elapsed = now.saturating_duration_since(entry.last).as_secs_f64();
        if elapsed > 0.0 {
            entry.tokens = (entry.tokens + elapsed * self.refill_per_sec).min(self.capacity);
            entry.last = now;
        }
        if entry.tokens >= 1.0 {
            entry.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::platforms::webchat::{
        WebChatProvider,
        bus::{EventBus, Subject},
        config::Config,
        session::WebchatSessionStore,
        types::{GreenticEvent, MessagePayload},
    };
    use axum::{
        Extension, Json,
        extract::{Path, Query},
        http::{self, HeaderMap, HeaderValue, StatusCode},
        response::IntoResponse,
    };
    use greentic_secrets::spec::{
        Scope, SecretUri, SecretsBackend, VersionedSecret, helpers::record_from_plain,
    };
    use serde_json::json;
    use tokio::sync::Mutex;

    #[derive(Clone)]
    struct TestSecretsBackend {
        secret: String,
    }

    impl TestSecretsBackend {
        fn new(secret: String) -> Self {
            Self { secret }
        }
    }

    impl SecretsBackend for TestSecretsBackend {
        fn put(
            &self,
            _record: greentic_secrets::spec::SecretRecord,
        ) -> greentic_secrets::spec::Result<greentic_secrets::spec::SecretVersion> {
            unimplemented!("test backend does not support writes")
        }

        fn get(
            &self,
            uri: &SecretUri,
            _version: Option<u64>,
        ) -> greentic_secrets::spec::Result<Option<VersionedSecret>> {
            if uri.category() == "webchat" && uri.name() == "jwt_signing_key" {
                let record = record_from_plain(self.secret.clone());
                Ok(Some(VersionedSecret {
                    version: 1,
                    deleted: false,
                    record: Some(record),
                }))
            } else {
                Ok(None)
            }
        }

        fn list(
            &self,
            _scope: &greentic_secrets::spec::Scope,
            _category_prefix: Option<&str>,
            _name_prefix: Option<&str>,
        ) -> greentic_secrets::spec::Result<Vec<greentic_secrets::spec::SecretListItem>> {
            unimplemented!("test backend does not support listing")
        }

        fn delete(
            &self,
            _uri: &SecretUri,
        ) -> greentic_secrets::spec::Result<greentic_secrets::spec::SecretVersion> {
            unimplemented!("test backend does not support delete")
        }

        fn versions(
            &self,
            _uri: &SecretUri,
        ) -> greentic_secrets::spec::Result<Vec<greentic_secrets::spec::SecretVersion>> {
            unimplemented!("test backend does not support versions")
        }

        fn exists(&self, uri: &SecretUri) -> greentic_secrets::spec::Result<bool> {
            Ok(uri.category() == "webchat" && uri.name() == "jwt_signing_key")
        }
    }

    fn test_provider(base_url: &str) -> WebChatProvider {
        let backend = Arc::new(TestSecretsBackend::new("test-signing-key".to_string()));
        let scope = Scope::new("global", "webchat", None).expect("valid signing scope");
        WebChatProvider::new(Config::with_base_url(base_url), backend).with_signing_scope(scope)
    }

    #[tokio::test]
    async fn user_activity_is_normalized_and_streamed() {
        let bus = Arc::new(RecordingBus::default());
        let sessions = Arc::new(MemorySessionStore::default());
        let conversations = memory_store();
        let provider = test_provider("http://localhost");
        let state = Arc::new(
            StandaloneState::with_store(
                provider,
                conversations.clone(),
                sessions.clone(),
                bus.clone(),
            )
            .await
            .expect("state"),
        );

        let user_token = issue_token(&state, "user-1").await;
        let (conversation_id, conversation_token) = start_conversation(&state, &user_token).await;

        let mut subscriber = conversations
            .subscribe(&conversation_id)
            .await
            .expect("subscribe");

        post_user_activity(
            &state,
            &conversation_id,
            &conversation_token,
            json!({
                "type": "message",
                "text": "hello"
            }),
        )
        .await;

        let stored = subscriber.recv().await.expect("activity");
        assert_eq!(stored.activity.text.as_deref(), Some("hello"));

        let events = bus.take().await;
        assert_eq!(events.len(), 1);
        match &events[0] {
            GreenticEvent::IncomingMessage(msg) => match &msg.payload {
                MessagePayload::Text { text, .. } => assert_eq!(text, "hello"),
                other => panic!("unexpected payload: {other:?}"),
            },
        }
    }

    #[tokio::test]
    async fn invoke_is_normalized_to_event() {
        let bus = Arc::new(RecordingBus::default());
        let sessions = Arc::new(MemorySessionStore::default());
        let conversations = memory_store();
        let provider = test_provider("http://localhost");
        let state = Arc::new(
            StandaloneState::with_store(provider, conversations.clone(), sessions, bus.clone())
                .await
                .expect("state"),
        );
        let user_token = issue_token(&state, "user-2").await;
        let (conversation_id, conversation_token) = start_conversation(&state, &user_token).await;

        post_user_activity(
            &state,
            &conversation_id,
            &conversation_token,
            json!({
                "type": "invoke",
                "name": "adaptiveCard/action",
                "value": {"foo": "bar"}
            }),
        )
        .await;

        let events = bus.take().await;
        assert_eq!(events.len(), 1);
        match &events[0] {
            GreenticEvent::IncomingMessage(msg) => match &msg.payload {
                MessagePayload::Event { name, .. } => assert_eq!(name, "adaptiveCard/action"),
                other => panic!("expected event payload, got {other:?}"),
            },
        }
    }

    #[tokio::test]
    async fn admin_bot_activity_appends_and_streams() {
        let bus = Arc::new(RecordingBus::default());
        let sessions = Arc::new(MemorySessionStore::default());
        let conversations = memory_store();
        let provider = test_provider("http://localhost");
        let state = Arc::new(
            StandaloneState::with_store(
                provider,
                conversations.clone(),
                sessions.clone(),
                bus.clone(),
            )
            .await
            .expect("state"),
        );

        let user_token = issue_token(&state, "user-3").await;
        let (conversation_id, _) = start_conversation(&state, &user_token).await;
        let mut subscriber = conversations
            .subscribe(&conversation_id)
            .await
            .expect("subscribe");

        let Json(response) = admin_post_activity_handler(
            Extension(Arc::clone(&state)),
            Path(AdminPath {
                env: "dev".to_string(),
                tenant: "acme".to_string(),
            }),
            Json(AdminPostActivityRequest {
                team: None,
                conversation_id: Some(conversation_id.clone()),
                activity: json!({
                    "type": "message",
                    "text": "bot says hi"
                }),
            }),
        )
        .await
        .expect("admin post");

        assert_eq!(response.posted, 1);
        assert_eq!(response.skipped, 0);

        let stored = subscriber.recv().await.expect("activity");
        assert_eq!(stored.activity.text.as_deref(), Some("bot says hi"));
        assert!(
            stored
                .activity
                .from
                .as_ref()
                .and_then(|from| from.role.as_deref())
                .map(|role| role.eq_ignore_ascii_case("bot"))
                .unwrap_or(false)
        );

        let session = sessions
            .get(&conversation_id)
            .await
            .expect("session fetch")
            .expect("session exists");
        assert_eq!(session.watermark.as_deref(), Some("1"));

        assert!(bus.take().await.is_empty());
    }

    async fn issue_token(state: &Arc<StandaloneState>, user_id: &str) -> String {
        let query = TenantQuery {
            env: "dev".to_string(),
            tenant: "acme".to_string(),
            team: None,
        };
        let body = GenerateTokenRequest {
            user: Some(UserDescriptor {
                id: Some(user_id.to_string()),
            }),
            trusted_origins: None,
        };
        let Json(response) = generate_token_handler(
            Extension(Arc::clone(state)),
            RemoteIp(None),
            Query(query),
            Json(body),
        )
        .await
        .expect("generate token");
        response.token
    }

    async fn start_conversation(state: &Arc<StandaloneState>, token: &str) -> (String, String) {
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
        let Json(response) = start_conversation_handler(Extension(Arc::clone(state)), headers)
            .await
            .expect("start conversation");
        (response.conversation_id, response.token)
    }

    async fn post_user_activity(
        state: &Arc<StandaloneState>,
        conversation_id: &str,
        token: &str,
        body: serde_json::Value,
    ) {
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
        headers.insert(
            http::header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        let response = post_activity_handler(
            Extension(Arc::clone(state)),
            Path(ConversationPath {
                id: conversation_id.to_string(),
            }),
            headers,
            Json(body),
        )
        .await
        .expect("post activity");
        let response = response.into_response();
        assert_eq!(response.status(), StatusCode::CREATED);
    }

    #[derive(Default)]
    struct RecordingBus {
        events: Mutex<Vec<GreenticEvent>>,
    }

    impl RecordingBus {
        async fn take(&self) -> Vec<GreenticEvent> {
            let mut guard = self.events.lock().await;
            std::mem::take(&mut *guard)
        }
    }

    #[async_trait::async_trait]
    impl EventBus for RecordingBus {
        async fn publish(&self, _subject: &Subject, event: &GreenticEvent) -> anyhow::Result<()> {
            self.events.lock().await.push(event.clone());
            Ok(())
        }
    }
}
