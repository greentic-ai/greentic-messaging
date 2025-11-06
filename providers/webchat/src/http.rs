use std::{collections::HashMap, future::Future, sync::Arc};

use axum::http::StatusCode;
use axum::{
    Json, Router,
    extract::{Path, State},
    routing::{get, post},
};
use metrics::counter;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::warn;

#[cfg(feature = "directline_standalone")]
use crate::conversation::{
    Activity, ChannelAccount, ConversationAccount, SharedConversationStore, StoreError, noop_store,
};
use crate::{
    WebChatProvider,
    auth::RouteContext,
    backoff,
    bus::{NoopBus, SharedBus},
    circuit::{CircuitBreaker, CircuitLabels, CircuitSettings},
    directline_client::{ConversationResponse, DirectLineApi, DirectLineError, TokenResponse},
    error::WebChatError,
    ingress::{
        IngressCtx, IngressDeps, ReqwestActivitiesTransport, SharedActivitiesTransport,
        run_poll_loop,
    },
    oauth::{GreenticOauthClient, ReqwestGreenticOauthClient},
    session::{MemorySessionStore, SharedSessionStore, WebchatSession},
    telemetry,
};
#[cfg(not(feature = "directline_standalone"))]
type SharedConversationStore = ();
use async_trait::async_trait;
use greentic_types::{EnvId, TeamId, TenantCtx, TenantId};
use reqwest::Client;
use tokio::{spawn, sync::Mutex as AsyncMutex};

pub struct AppState {
    pub provider: WebChatProvider,
    pub direct_line: Arc<dyn DirectLineApi>,
    pub http_client: Client,
    pub transport: SharedActivitiesTransport,
    pub bus: SharedBus,
    pub sessions: SharedSessionStore,
    pub activity_poster: SharedDirectLinePoster,
    pub oauth_client: Arc<dyn GreenticOauthClient>,
    #[cfg(feature = "directline_standalone")]
    pub conversations: SharedConversationStore,
    circuit_settings: CircuitSettings,
    token_circuits: CircuitRegistry,
    conversation_circuits: CircuitRegistry,
}

impl Clone for AppState {
    fn clone(&self) -> Self {
        Self {
            provider: self.provider.clone(),
            direct_line: Arc::clone(&self.direct_line),
            http_client: self.http_client.clone(),
            transport: Arc::clone(&self.transport),
            bus: Arc::clone(&self.bus),
            sessions: Arc::clone(&self.sessions),
            activity_poster: Arc::clone(&self.activity_poster),
            oauth_client: Arc::clone(&self.oauth_client),
            #[cfg(feature = "directline_standalone")]
            conversations: Arc::clone(&self.conversations),
            circuit_settings: self.circuit_settings.clone(),
            token_circuits: self.token_circuits.clone(),
            conversation_circuits: self.conversation_circuits.clone(),
        }
    }
}

impl AppState {
    pub fn new(
        provider: WebChatProvider,
        direct_line: Arc<dyn DirectLineApi>,
        http_client: Client,
    ) -> Self {
        let transport_client = http_client.clone();
        let poster_client = http_client.clone();
        let oauth_client_http = http_client.clone();
        let transport: SharedActivitiesTransport =
            Arc::new(ReqwestActivitiesTransport::new(transport_client));
        let activity_poster: SharedDirectLinePoster =
            Arc::new(HttpDirectLinePoster::new(poster_client));
        let oauth_client: Arc<dyn GreenticOauthClient> =
            Arc::new(ReqwestGreenticOauthClient::new(oauth_client_http));
        let circuit_settings = CircuitSettings::default();
        Self {
            provider,
            direct_line,
            http_client,
            transport,
            bus: Arc::new(NoopBus),
            sessions: Arc::new(MemorySessionStore::default()),
            activity_poster,
            oauth_client,
            #[cfg(feature = "directline_standalone")]
            conversations: noop_store(),
            circuit_settings: circuit_settings.clone(),
            token_circuits: CircuitRegistry::new(circuit_settings.clone()),
            conversation_circuits: CircuitRegistry::new(circuit_settings),
        }
    }

    pub fn with_bus(mut self, bus: SharedBus) -> Self {
        self.bus = bus;
        self
    }

    pub fn with_sessions(mut self, sessions: SharedSessionStore) -> Self {
        self.sessions = sessions;
        self
    }

    pub fn with_transport(mut self, transport: SharedActivitiesTransport) -> Self {
        self.transport = transport;
        self
    }

    pub fn with_activity_poster(mut self, poster: SharedDirectLinePoster) -> Self {
        self.activity_poster = poster;
        self
    }

    pub fn with_oauth_client(mut self, client: Arc<dyn GreenticOauthClient>) -> Self {
        self.oauth_client = client;
        self
    }

    #[cfg(feature = "directline_standalone")]
    pub fn with_conversations(mut self, conversations: SharedConversationStore) -> Self {
        self.conversations = conversations;
        self
    }

    pub async fn post_activity(
        &self,
        conversation_id: &str,
        bearer_token: &str,
        activity: Value,
    ) -> Result<(), WebChatError> {
        validate_activity_for_post(&activity)?;
        self.activity_poster
            .post_activity(
                self.provider.config().direct_line_base(),
                conversation_id,
                bearer_token,
                activity,
            )
            .await
            .map_err(WebChatError::from)
    }
}

pub fn router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/webchat/healthz", get(healthz))
        .route(
            "/webchat/{env}/{tenant}/tokens/generate",
            post(generate_token),
        )
        .route(
            "/webchat/{env}/{tenant}/{team}/tokens/generate",
            post(generate_token),
        )
        .route(
            "/webchat/{env}/{tenant}/conversations/start",
            post(start_conversation),
        )
        .route(
            "/webchat/{env}/{tenant}/{team}/conversations/start",
            post(start_conversation),
        )
        .route(
            "/webchat/admin/{env}/{tenant}/post-activity",
            post(admin_post_activity),
        )
        .route("/webchat/oauth/start", get(crate::oauth::start))
        .route("/webchat/oauth/callback", get(crate::oauth::callback))
        .with_state(state)
}

async fn healthz() -> StatusCode {
    StatusCode::NO_CONTENT
}

#[derive(Deserialize)]
struct TokenPath {
    env: String,
    tenant: String,
    #[serde(default)]
    team: Option<String>,
}

#[derive(Default, Deserialize)]
struct GenerateTokenRequestBody {}

#[derive(Serialize)]
struct GenerateTokenResponse {
    token: String,
    expires_in: u64,
}

#[derive(Deserialize)]
struct StartConversationRequest {
    token: String,
}

#[derive(Serialize)]
struct StartConversationResponse {
    token: String,
    #[serde(rename = "conversationId")]
    conversation_id: String,
    #[serde(rename = "streamUrl", skip_serializing_if = "Option::is_none")]
    stream_url: Option<String>,
    expires_in: u64,
}

async fn generate_token(
    State(state): State<AppState>,
    Path(path): Path<TokenPath>,
    Json(_): Json<GenerateTokenRequestBody>,
) -> Result<Json<GenerateTokenResponse>, WebChatError> {
    let ctx = RouteContext::new(path.env, path.tenant, path.team);
    let span = telemetry::span_for("tokens.generate", &ctx);
    let _guard = span.enter();
    let team_label = telemetry::team_or_dash(ctx.team());
    let env_metric = ctx.env().to_string();
    let tenant_metric = ctx.tenant().to_string();
    let team_metric = team_label.to_string();
    let tenant_ctx = build_tenant_ctx(&ctx);

    let secret = state
        .provider
        .direct_line_secret(&tenant_ctx)
        .await
        .map_err(|err| {
            counter!(
                "webchat_errors_total",
                "kind" => "secret_backend_error",
                "env" => env_metric.clone(),
                "tenant" => tenant_metric.clone(),
                "team" => team_metric.clone()
            )
            .increment(1);
            WebChatError::Internal(err)
        })?
        .ok_or_else(|| {
            counter!(
                "webchat_errors_total",
                "kind" => "missing_secret",
                "env" => env_metric.clone(),
                "tenant" => tenant_metric.clone(),
                "team" => team_metric.clone()
            )
            .increment(1);
            WebChatError::MissingSecret
        })?;

    let secret = Arc::new(secret);
    let response = call_with_circuit(
        &state.token_circuits,
        direct_line_circuit_key("tokens", &ctx),
        direct_line_labels(&ctx),
        {
            let direct_line = Arc::clone(&state.direct_line);
            let secret = Arc::clone(&secret);
            move || {
                let direct_line = Arc::clone(&direct_line);
                let secret = Arc::clone(&secret);
                async move { direct_line.generate_token(secret.as_ref()).await }
            }
        },
    )
    .await
    .map_err(|err| map_directline_error("tokens.generate", err))?;

    counter!(
        "webchat_tokens_generated_total",
        "env" => env_metric.clone(),
        "tenant" => tenant_metric.clone(),
        "team" => team_metric.clone()
    )
    .increment(1);

    Ok(Json(token_response_body(response)))
}

async fn start_conversation(
    State(state): State<AppState>,
    Path(path): Path<TokenPath>,
    Json(body): Json<StartConversationRequest>,
) -> Result<Json<StartConversationResponse>, WebChatError> {
    let ctx = RouteContext::new(path.env, path.tenant, path.team);
    let span = telemetry::span_for("conversations.start", &ctx);
    let _guard = span.enter();
    let env_metric = ctx.env().to_string();
    let tenant_metric = ctx.tenant().to_string();
    let team_metric = telemetry::team_or_dash(ctx.team()).to_string();

    let trimmed = body.token.trim();
    if trimmed.is_empty() {
        return Err(WebChatError::BadRequest("token is required"));
    }

    let token_for_retry = Arc::new(trimmed.to_string());
    let conversation_response = call_with_circuit(
        &state.conversation_circuits,
        direct_line_circuit_key("conversations", &ctx),
        direct_line_labels(&ctx),
        {
            let direct_line = Arc::clone(&state.direct_line);
            let token = Arc::clone(&token_for_retry);
            move || {
                let direct_line = Arc::clone(&direct_line);
                let token = Arc::clone(&token);
                async move { direct_line.start_conversation(token.as_ref()).await }
            }
        },
    )
    .await
    .map_err(|err| map_directline_error("conversations.start", err))?;

    let tenant_ctx = build_tenant_ctx(&ctx);
    state
        .sessions
        .upsert(WebchatSession::new(
            conversation_response.conversation_id.clone(),
            tenant_ctx.clone(),
            conversation_response.token.clone(),
        ))
        .await
        .map_err(WebChatError::Internal)?;

    counter!(
        "webchat_conversations_started_total",
        "env" => env_metric.clone(),
        "tenant" => tenant_metric.clone(),
        "team" => team_metric.clone()
    )
    .increment(1);
    let ingress_deps = IngressDeps {
        bus: Arc::clone(&state.bus),
        sessions: Arc::clone(&state.sessions),
        dl_base: state.provider.config().direct_line_base().to_string(),
        transport: Arc::clone(&state.transport),
        circuit: state.circuit_settings.clone(),
    };
    let ingress_ctx = IngressCtx {
        tenant_ctx,
        conversation_id: conversation_response.conversation_id.clone(),
        token: conversation_response.token.clone(),
    };

    spawn(async move {
        if let Err(err) = run_poll_loop(ingress_deps, ingress_ctx).await {
            warn!(error = %err, "webchat poll loop terminated");
        }
    });

    Ok(Json(conversation_response_body(conversation_response)))
}

fn token_response_body(response: TokenResponse) -> GenerateTokenResponse {
    GenerateTokenResponse {
        token: response.token,
        expires_in: response.expires_in.unwrap_or(DEFAULT_EXPIRY_SECONDS),
    }
}

fn conversation_response_body(response: ConversationResponse) -> StartConversationResponse {
    StartConversationResponse {
        token: response.token,
        conversation_id: response.conversation_id,
        stream_url: response.stream_url,
        expires_in: response.expires_in.unwrap_or(DEFAULT_EXPIRY_SECONDS),
    }
}

const DEFAULT_EXPIRY_SECONDS: u64 = 1800;

fn build_tenant_ctx(ctx: &RouteContext) -> TenantCtx {
    let mut tenant_ctx = TenantCtx::new(
        EnvId::from(ctx.env().to_string()),
        TenantId::from(ctx.tenant().to_string()),
    );
    if let Some(team) = ctx.team() {
        tenant_ctx = tenant_ctx.with_team(Some(TeamId::from(team.to_string())));
    }
    tenant_ctx
}

fn map_directline_error(action: &str, err: DirectLineError) -> WebChatError {
    log_directline_error(action, &err);
    WebChatError::from(err)
}

const MAX_DIRECT_LINE_RETRIES: u32 = 5;
const NO_CONVERSATION_LABEL: &str = "-";

#[derive(Clone)]
struct CircuitRegistry {
    settings: CircuitSettings,
    inner: Arc<AsyncMutex<HashMap<String, Arc<AsyncMutex<CircuitBreaker>>>>>,
}

impl CircuitRegistry {
    fn new(settings: CircuitSettings) -> Self {
        Self {
            settings,
            inner: Arc::new(AsyncMutex::new(HashMap::new())),
        }
    }

    async fn circuit(&self, key: &str, labels: CircuitLabels) -> Arc<AsyncMutex<CircuitBreaker>> {
        let mut guard = self.inner.lock().await;
        if let Some(existing) = guard.get(key) {
            existing.clone()
        } else {
            let circuit = Arc::new(AsyncMutex::new(CircuitBreaker::new(
                self.settings.clone(),
                labels,
            )));
            guard.insert(key.to_string(), circuit.clone());
            circuit
        }
    }
}

fn direct_line_circuit_key(scope: &str, ctx: &RouteContext) -> String {
    let team = telemetry::team_or_dash(ctx.team());
    let team_slug = team.to_ascii_lowercase();
    format!(
        "{scope}:{env}:{tenant}:{team}",
        scope = scope,
        env = ctx.env().to_ascii_lowercase(),
        tenant = ctx.tenant().to_ascii_lowercase(),
        team = team_slug
    )
}

fn direct_line_labels(ctx: &RouteContext) -> CircuitLabels {
    CircuitLabels::new(
        ctx.env().to_string(),
        ctx.tenant().to_string(),
        telemetry::team_or_dash(ctx.team()).to_string(),
        NO_CONVERSATION_LABEL.to_string(),
    )
}

fn is_retryable(err: &DirectLineError) -> bool {
    matches!(err, DirectLineError::Transport(_))
        || matches!(err, DirectLineError::Remote { status, .. }
            if *status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error())
}

async fn call_with_circuit<F, Fut, T>(
    registry: &CircuitRegistry,
    key: String,
    labels: CircuitLabels,
    mut operation: F,
) -> Result<T, DirectLineError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, DirectLineError>>,
{
    let circuit = registry.circuit(&key, labels).await;
    let mut attempt: u32 = 0;
    loop {
        {
            let mut guard = circuit.lock().await;
            guard.before_request().await;
        }

        match operation().await {
            Ok(value) => {
                let mut guard = circuit.lock().await;
                guard.on_success();
                return Ok(value);
            }
            Err(err) => {
                let retryable = is_retryable(&err);
                {
                    let mut guard = circuit.lock().await;
                    guard.on_failure();
                }

                if retryable && attempt < MAX_DIRECT_LINE_RETRIES {
                    attempt = attempt.saturating_add(1);
                    if let DirectLineError::Remote {
                        retry_after: Some(delay),
                        ..
                    } = &err
                    {
                        tokio::time::sleep(*delay).await;
                    } else {
                        backoff::sleep(attempt).await;
                    }
                    continue;
                }

                return Err(err);
            }
        }
    }
}

fn log_directline_error(action: &str, err: &DirectLineError) {
    match err {
        DirectLineError::Remote {
            status,
            retry_after,
            ..
        } => {
            let status_label = status.as_str().to_string();
            let endpoint_label = action.to_string();
            warn!(%action, %status, retry_after = retry_after.map(|d| d.as_secs()), "direct line remote error");
            counter!(
                "webchat_errors_total",
                "kind" => "directline_remote",
                "endpoint" => endpoint_label,
                "status" => status_label
            )
            .increment(1);
        }
        DirectLineError::Transport(source) => {
            warn!(%action, error = %source, "direct line transport error");
            counter!(
                "webchat_errors_total",
                "kind" => "directline_transport",
                "endpoint" => action.to_string()
            )
            .increment(1);
        }
        DirectLineError::Decode(_) => {
            warn!(%action, "direct line decode error");
            counter!(
                "webchat_errors_total",
                "kind" => "directline_decode",
                "endpoint" => action.to_string()
            )
            .increment(1);
        }
        DirectLineError::Config(_) => {
            warn!(%action, "direct line configuration error");
            counter!(
                "webchat_errors_total",
                "kind" => "directline_config",
                "endpoint" => action.to_string()
            )
            .increment(1);
        }
    }
}

const ALLOWED_ATTACHMENT_TYPES: &[&str] = &[
    "application/vnd.microsoft.card.adaptive",
    "application/json",
    "image/png",
    "image/jpeg",
    "image/gif",
];

const MAX_ATTACHMENT_BYTES: usize = 512 * 1024;

fn validate_activity_for_post(activity: &Value) -> Result<(), WebChatError> {
    if let Some(attachments) = activity.get("attachments").and_then(Value::as_array) {
        for attachment in attachments {
            let content_type = attachment
                .get("contentType")
                .or_else(|| attachment.get("content_type"))
                .and_then(Value::as_str)
                .ok_or(WebChatError::BadRequest("attachment missing contentType"))?;

            let allowed = ALLOWED_ATTACHMENT_TYPES
                .iter()
                .any(|allowed| content_type.eq_ignore_ascii_case(allowed));
            if !allowed {
                return Err(WebChatError::BadRequest(
                    "attachment content type not allowed",
                ));
            }

            if let Some(content) = attachment.get("content") {
                let serialized = serde_json::to_vec(content).map_err(|_| {
                    WebChatError::BadRequest("attachment content is not valid JSON")
                })?;
                if serialized.len() > MAX_ATTACHMENT_BYTES {
                    return Err(WebChatError::BadRequest(
                        "attachment content exceeds size limit",
                    ));
                }
            }
        }
    }

    Ok(())
}

#[derive(Deserialize)]
pub struct AdminPath {
    pub env: String,
    pub tenant: String,
}

#[derive(Deserialize)]
pub struct AdminPostActivityRequest {
    #[serde(default)]
    pub team: Option<String>,
    #[serde(rename = "conversation_id", default)]
    pub conversation_id: Option<String>,
    pub activity: Value,
}

#[derive(Serialize)]
pub struct AdminPostActivityResponse {
    pub posted: usize,
    pub skipped: usize,
}

pub async fn admin_post_activity(
    State(state): State<AppState>,
    Path(path): Path<AdminPath>,
    Json(body): Json<AdminPostActivityRequest>,
) -> Result<Json<AdminPostActivityResponse>, WebChatError> {
    let AdminPostActivityRequest {
        team,
        conversation_id,
        activity,
    } = body;

    if !activity.is_object() {
        return Err(WebChatError::BadRequest("activity must be an object"));
    }

    let team_filter = team.as_deref();

    if let Some(conversation_id) = conversation_id.as_deref() {
        let session = state
            .sessions
            .get(conversation_id)
            .await
            .map_err(WebChatError::Internal)?
            .ok_or(WebChatError::NotFound("conversation not found"))?;

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
            return Err(WebChatError::NotFound("conversation not found"));
        }

        if let Some(team) = team_filter
            && !session
                .tenant_ctx
                .team
                .as_ref()
                .map(|value| value.as_ref().eq_ignore_ascii_case(team))
                .unwrap_or(false)
        {
            return Err(WebChatError::NotFound("conversation not found"));
        }

        return append_and_broadcast(&state, &session, activity)
            .await
            .map(|posted| Json(AdminPostActivityResponse { posted, skipped: 0 }));
    }

    let sessions = state
        .sessions
        .list_by_tenant(&path.env, &path.tenant, team_filter)
        .await
        .map_err(WebChatError::Internal)?;

    if sessions.is_empty() {
        return Err(WebChatError::NotFound("no matching sessions"));
    }

    let mut posted = 0usize;
    let mut skipped = 0usize;
    for session in sessions {
        match append_and_broadcast(&state, &session, activity.clone()).await {
            Ok(count) => posted += count,
            Err(WebChatError::BadRequest(_)) => {
                skipped += 1;
                continue;
            }
            Err(other) => return Err(other),
        }
    }

    Ok(Json(AdminPostActivityResponse { posted, skipped }))
}

#[cfg(feature = "directline_standalone")]
async fn append_and_broadcast(
    state: &AppState,
    session: &WebchatSession,
    activity_json: Value,
) -> Result<usize, WebChatError> {
    if !session.proactive_ok {
        return Err(WebChatError::BadRequest("proactive messaging disabled"));
    }
    let mut activity: Activity = serde_json::from_value(activity_json)
        .map_err(|_| WebChatError::BadRequest("activity must match Bot Framework schema"))?;
    apply_bot_defaults(&mut activity, &session.conversation_id);

    let stored = match state
        .conversations
        .append(&session.conversation_id, activity.clone())
        .await
    {
        Ok(stored) => stored,
        Err(StoreError::NotFound(_)) => {
            state
                .conversations
                .create(&session.conversation_id, session.tenant_ctx.clone())
                .await
                .map_err(|err| WebChatError::Internal(err.into()))?;
            state
                .conversations
                .append(&session.conversation_id, activity)
                .await
                .map_err(|err| WebChatError::Internal(err.into()))?
        }
        Err(StoreError::QuotaExceeded(_)) => {
            return Err(WebChatError::BadRequest(
                "conversation backlog quota exceeded",
            ));
        }
        Err(err) => return Err(WebChatError::Internal(err.into())),
    };

    if let Err(err) = state
        .sessions
        .update_watermark(
            &session.conversation_id,
            Some((stored.watermark + 1).to_string()),
        )
        .await
    {
        warn!(error = %err, "failed to update watermark");
    }

    Ok(1)
}

#[cfg(feature = "directline_standalone")]
fn apply_bot_defaults(activity: &mut Activity, conversation_id: &str) {
    match activity.from.as_mut() {
        Some(from) => {
            if from.id.trim().is_empty() {
                from.id = "bot".into();
            }
            from.role = Some("bot".into());
        }
        None => {
            activity.from = Some(ChannelAccount {
                id: "bot".into(),
                name: None,
                role: Some("bot".into()),
            });
        }
    }
    if activity
        .conversation
        .as_ref()
        .map(|conv| conv.id.trim().is_empty())
        .unwrap_or(true)
    {
        activity.conversation = Some(ConversationAccount {
            id: conversation_id.to_string(),
        });
    }
}

#[cfg(not(feature = "directline_standalone"))]
async fn append_and_broadcast(
    state: &AppState,
    session: &WebchatSession,
    activity: Value,
) -> Result<usize, WebChatError> {
    if !session.proactive_ok {
        return Err(WebChatError::BadRequest("proactive messaging disabled"));
    }
    state
        .post_activity(&session.conversation_id, &session.bearer_token, activity)
        .await?;
    Ok(1)
}

pub type SharedDirectLinePoster = Arc<dyn DirectLinePoster>;

#[async_trait]
pub trait DirectLinePoster: Send + Sync {
    async fn post_activity(
        &self,
        base_url: &str,
        conversation_id: &str,
        bearer_token: &str,
        activity: Value,
    ) -> Result<(), DirectLineError>;
}

pub struct HttpDirectLinePoster {
    client: Client,
}

impl HttpDirectLinePoster {
    pub fn new(client: Client) -> Self {
        Self { client }
    }
}

#[async_trait]
impl DirectLinePoster for HttpDirectLinePoster {
    async fn post_activity(
        &self,
        base_url: &str,
        conversation_id: &str,
        bearer_token: &str,
        activity: Value,
    ) -> Result<(), DirectLineError> {
        let url = format!(
            "{}/conversations/{}/activities",
            base_url.trim_end_matches('/'),
            conversation_id
        );

        let response = self
            .client
            .post(url)
            .bearer_auth(bearer_token)
            .json(&activity)
            .send()
            .await
            .map_err(DirectLineError::Transport)?;

        let status = response.status();
        if status.is_success() {
            return Ok(());
        }

        let retry_after = response
            .headers()
            .get(axum::http::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .map(std::time::Duration::from_secs);
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "<unreadable>".to_string());
        let message = if body.len() > 512 {
            body[..512].to_string()
        } else {
            body
        };

        Err(DirectLineError::Remote {
            status,
            retry_after,
            message,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn healthz_returns_no_content() {
        assert_eq!(healthz().await, StatusCode::NO_CONTENT);
    }
}
