//! WebChat egress adapter that routes outbound messages to in-browser SSE
//! clients keyed by `(env, tenant, team, user)`.

use anyhow::{Result, anyhow};
use async_nats::jetstream::AckKind;
use async_stream::stream;
use axum::{
    Router,
    extract::{Query, State},
    response::{Html, IntoResponse, sse::Event, sse::Sse},
    routing::get,
};
use dashmap::DashMap;
use futures::StreamExt;
use gsm_backpressure::BackpressureLimiter;
use gsm_core::messaging_card::ensure_oauth_start_url;
use gsm_core::messaging_card::{MessageCardEngine, MessageCardKind, RenderSpec};
use gsm_core::oauth::{OauthClient, ReqwestTransport};
use gsm_core::{AdaptiveMessageCard, OutMessage, Platform, TenantCtx};
use gsm_dlq::{DlqError, DlqPublisher};
use gsm_egress_common::{
    egress::bootstrap,
    telemetry::{
        AuthRenderMode, context_from_out, record_auth_card_render, record_egress_success,
        start_acquire_span, start_send_span,
    },
};
use gsm_telemetry::{MessageContext, install as init_telemetry};
use gsm_translator::{Translator, WebChatTranslator};
use include_dir::{Dir, include_dir};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::broadcast;
use tracing::{Instrument, Level, event, warn};

static ASSETS: Dir = include_dir!("$CARGO_MANIFEST_DIR/static");

#[derive(Clone)]
struct AppState {
    sessions: Arc<DashMap<SessionKey, broadcast::Sender<Value>>>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            sessions: Arc::new(DashMap::new()),
        }
    }
}

impl AppState {
    fn subscribe(&self, key: SessionKey) -> broadcast::Receiver<Value> {
        let sender = self.sessions.entry(key).or_insert_with(|| {
            let (tx, _rx) = broadcast::channel(64);
            tx
        });
        sender.subscribe()
    }

    fn publish(&self, ctx: &TenantCtx, payload: &Value) -> bool {
        let key = SessionKey::from_ctx(ctx);
        let mut delivered = false;
        for candidate in key.fallbacks() {
            if let Some(sender) = self.sessions.get(&candidate) {
                let _ = sender.send(payload.clone());
                delivered = true;
            }
        }
        delivered
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct SessionKey {
    env: String,
    tenant: String,
    team: Option<String>,
    user: Option<String>,
}

impl SessionKey {
    fn from_ctx(ctx: &TenantCtx) -> Self {
        Self {
            env: ctx.env.as_str().to_string(),
            tenant: ctx.tenant.as_str().to_string(),
            team: ctx.team.as_ref().map(|team| team.as_str().to_string()),
            user: ctx.user.as_ref().map(|user| user.as_str().to_string()),
        }
    }

    fn from_query(query: &EventsQuery) -> Self {
        let env = query
            .env
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(default_env);
        let tenant = query
            .tenant
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(default_tenant);
        let team = query.team.clone().filter(|s| !s.is_empty());
        let user = query.user.clone().filter(|s| !s.is_empty());
        Self {
            env,
            tenant,
            team,
            user,
        }
    }

    fn fallbacks(&self) -> Vec<SessionKey> {
        let mut keys = vec![self.clone()];
        if self.user.is_some() {
            let mut without_user = self.clone();
            without_user.user = None;
            push_unique(&mut keys, without_user);
        }
        if self.team.is_some() {
            let tenant_key = SessionKey {
                env: self.env.clone(),
                tenant: self.tenant.clone(),
                team: None,
                user: None,
            };
            push_unique(&mut keys, tenant_key);
        }
        keys
    }
}

fn push_unique(list: &mut Vec<SessionKey>, key: SessionKey) {
    if !list.contains(&key) {
        list.push(key);
    }
}

fn default_env() -> String {
    std::env::var("GREENTIC_ENV").unwrap_or_else(|_| "dev".into())
}

fn default_tenant() -> String {
    std::env::var("TENANT").unwrap_or_else(|_| "acme".into())
}

#[derive(Debug, Deserialize)]
struct EventsQuery {
    #[serde(default)]
    tenant: Option<String>,
    #[serde(default)]
    team: Option<String>,
    #[serde(default)]
    user: Option<String>,
    #[serde(default)]
    env: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    init_telemetry("greentic-messaging")?;
    let nats_url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
    let env = default_env();
    let platform = std::env::var("PLATFORM").unwrap_or_else(|_| "webchat".into());

    let queue = bootstrap(&nats_url, &env, &platform).await?;
    tracing::info!(
        stream = %queue.stream,
        consumer = %queue.consumer,
        "egress-webchat consuming from JetStream"
    );

    let state = AppState::default();
    let runner_state = state.clone();
    let card_engine = Arc::new(MessageCardEngine::bootstrap());
    let oauth_client = match std::env::var("OAUTH_BASE_URL") {
        Ok(_) => match OauthClient::from_env(reqwest::Client::new()) {
            Ok(client) => {
                tracing::info!("OAUTH_BASE_URL detected; WebChat OAuth builder enabled");
                Some(Arc::new(client))
            }
            Err(err) => {
                tracing::warn!(error = %err, "failed to initialize OAuth client");
                None
            }
        },
        Err(_) => None,
    };
    let client = queue.client();
    let messages = queue.messages;
    let limiter = queue.limiter;
    let dlq = DlqPublisher::new("egress", client).await?;

    tokio::spawn({
        let card_engine = card_engine.clone();
        let oauth_client = oauth_client.clone();
        async move {
            let translator = WebChatTranslator::new();
            let dlq = dlq.clone();
            let mut messages = messages;
            while let Some(next) = messages.next().await {
                let msg = match next {
                    Ok(msg) => msg,
                    Err(err) => {
                        tracing::error!(error = %err, "jetstream message error");
                        continue;
                    }
                };

                let out: OutMessage = match serde_json::from_slice(msg.payload.as_ref()) {
                    Ok(v) => v,
                    Err(err) => {
                        tracing::warn!(error = %err, "bad out msg");
                        let _ = msg.ack().await;
                        continue;
                    }
                };

                if out.platform != Platform::WebChat {
                    if let Err(err) = msg.ack().await {
                        tracing::error!(error = %err, "ack failed");
                    }
                    continue;
                }

                let ctx = context_from_out(&out);
                let msg_id = ctx
                    .labels
                    .msg_id
                    .clone()
                    .unwrap_or_else(|| out.message_id());

                let acquire_span = start_acquire_span(&ctx);
                let _permit = match limiter.acquire(&out.tenant).instrument(acquire_span).await {
                    Ok(p) => p,
                    Err(err) => {
                        tracing::error!(
                            error = %err,
                            tenant = %ctx.labels.tenant,
                            platform = "webchat",
                            "failed to acquire backpressure permit"
                        );
                        let _ = msg.ack_with(AckKind::Nak(None)).await;
                        continue;
                    }
                };
                event!(
                    Level::INFO,
                    tenant = %ctx.labels.tenant,
                    platform = "webchat",
                    msg_id = %msg_id,
                    acquired = true,
                    "backpressure permit acquired"
                );

                let translation = translate_payloads(
                    &translator,
                    &out,
                    &ctx,
                    card_engine.as_ref(),
                    oauth_client.as_deref(),
                )
                .await;

                let result = translation.map(|payloads| {
                    let send_span = start_send_span(&ctx);
                    let send_start = Instant::now();
                    {
                        let _guard = send_span.enter();
                        for payload in payloads {
                            if !runner_state.publish(&out.ctx, &payload) {
                                tracing::debug!(
                                    tenant = %out.ctx.tenant.as_str(),
                                    team = ?out.ctx.team.as_ref().map(|t| t.as_str()),
                                    user = ?out.ctx.user.as_ref().map(|u| u.as_str()),
                                    "no active webchat session"
                                );
                            }
                        }
                    }
                    send_start.elapsed().as_secs_f64() * 1000.0
                });
                match result {
                    Ok(latency_ms) => {
                        if let Err(err) = msg.ack().await {
                            tracing::error!(error = %err, "ack failed");
                        }
                        record_egress_success(&ctx, latency_ms);
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, "webchat translation failed");
                        if let Err(dlq_err) = dlq
                            .publish(
                                &out.tenant,
                                out.platform.as_str(),
                                &msg_id,
                                1,
                                DlqError {
                                    code: "E_TRANSLATE".into(),
                                    message: err.to_string(),
                                    stage: None,
                                },
                                &out,
                            )
                            .await
                        {
                            tracing::error!(error = %dlq_err, "failed to publish dlq entry");
                        }
                        if let Err(nak_err) = msg.ack_with(AckKind::Nak(None)).await {
                            tracing::error!(error = %nak_err, "nak failed");
                        }
                    }
                }
            }
        }
    });

    let app = Router::new()
        .route("/", get(index))
        .route("/events", get(events))
        .with_state(state);

    let addr: std::net::SocketAddr = std::env::var("BIND")
        .unwrap_or_else(|_| "0.0.0.0:8070".into())
        .parse()
        .unwrap();
    tracing::info!("egress-webchat UI on {}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn translate_payloads(
    translator: &WebChatTranslator,
    out: &OutMessage,
    ctx: &MessageContext,
    engine: &MessageCardEngine,
    oauth_client: Option<&OauthClient<ReqwestTransport>>,
) -> Result<Vec<Value>> {
    if let Some(card) = &out.adaptive_card {
        match render_webchat_card(out, ctx, card, engine, oauth_client).await {
            Ok(value) => return Ok(vec![value]),
            Err(err) => {
                warn!(error = %err, "failed to render adaptive MessageCard; using legacy translator");
            }
        }
    }
    translator.to_platform(out)
}

async fn render_webchat_card(
    out: &OutMessage,
    ctx: &MessageContext,
    card: &AdaptiveMessageCard,
    engine: &MessageCardEngine,
    oauth_client: Option<&OauthClient<ReqwestTransport>>,
) -> Result<Value> {
    let mut working = card.clone();
    if matches!(working.kind, MessageCardKind::Oauth) {
        let has_url = working
            .oauth
            .as_ref()
            .and_then(|oauth| oauth.start_url.as_ref())
            .is_some();
        if let Some(client) = oauth_client {
            ensure_oauth_start_url(&mut working, &out.ctx, client, None).await?;
        } else if !has_url {
            return Err(anyhow!(
                "oauth card missing start_url and OAuth builder not configured"
            ));
        }
    }

    let spec = engine
        .render_spec(&working)
        .map_err(|err| anyhow!("message card normalization failed: {err}"))?;
    let snapshot = engine
        .render_snapshot_tracked("webchat", &spec)
        .ok_or_else(|| anyhow!("webchat renderer unavailable"))?;

    if matches!(spec, RenderSpec::Auth(_))
        && let Some(oauth) = working.oauth.as_ref()
    {
        let mode = if snapshot.ir.is_none() {
            AuthRenderMode::Native
        } else {
            AuthRenderMode::Downgrade
        };
        let team = out.ctx.team.as_ref().map(|team| team.as_ref());
        record_auth_card_render(
            ctx,
            oauth.provider.as_str(),
            mode,
            oauth.connection_name.as_deref(),
            oauth.start_url.as_deref(),
            team,
        );
    }

    let payload = snapshot.output.payload;
    Ok(payload)
}

async fn index() -> Html<String> {
    let file = ASSETS.get_file("index.html").expect("index.html missing");
    Html(String::from_utf8_lossy(file.contents()).to_string())
}

async fn events(
    Query(query): Query<EventsQuery>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let key = SessionKey::from_query(&query);
    let mut rx = state.subscribe(key);
    let stream = stream! {
        while let Ok(v) = rx.recv().await {
            let event = Event::default().data(v.to_string());
            yield Ok::<_, std::io::Error>(event);
        }
    };
    Sse::new(stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallbacks_drop_user_then_team() {
        let key = SessionKey {
            env: "dev".into(),
            tenant: "acme".into(),
            team: Some("support".into()),
            user: Some("u-1".into()),
        };
        let keys = key.fallbacks();
        assert_eq!(keys.len(), 3);
        assert_eq!(keys[0].user.as_deref(), Some("u-1"));
        assert_eq!(keys[1].user, None);
        assert_eq!(keys[1].team.as_deref(), Some("support"));
        assert_eq!(keys[2].team, None);
    }
}
