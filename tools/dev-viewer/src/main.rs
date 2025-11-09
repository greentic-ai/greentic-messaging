use std::collections::BTreeMap;
use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path as AxumPath, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use axum::{Json, Router};
use clap::Parser;
use gsm_core::messaging_card::adaptive::validator::validate_ac_json;
use gsm_core::messaging_card::ir::{MessageCardIr, Meta};
use gsm_core::messaging_card::renderers::RenderOutput;
use gsm_core::messaging_card::{
    AuthRenderSpec, MessageCard, MessageCardEngine, MessageCardKind, RenderIntent, RenderSnapshot,
    RenderSpec, ensure_oauth_start_url,
};
use gsm_core::oauth::{OauthClient, ReqwestTransport};
use gsm_core::{TenantCtx, Tier, make_tenant_ctx};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::signal;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(author, version, about = "Preview MessageCards across platforms", long_about = None)]
struct Opts {
    /// Address to bind the dev viewer (e.g. 127.0.0.1:7878)
    #[arg(long, default_value = "127.0.0.1:7878")]
    listen: SocketAddr,

    /// Directory containing MessageCard fixtures
    #[arg(long, default_value = "libs/core/tests/fixtures/cards")]
    fixtures: PathBuf,
}

#[derive(Clone)]
struct AppState {
    engine: Arc<MessageCardEngine>,
    fixtures_dir: Arc<PathBuf>,
    platforms: Vec<String>,
    tenant_ctx: Arc<TenantCtx>,
    oauth_client: Option<Arc<OauthClient<ReqwestTransport>>>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let opts = Opts::parse();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let fixtures_dir = opts
        .fixtures
        .canonicalize()
        .unwrap_or_else(|_| opts.fixtures.clone());
    if !fixtures_dir.is_dir() {
        anyhow::bail!("fixtures directory {:?} does not exist", fixtures_dir);
    }

    let engine = MessageCardEngine::bootstrap();
    let platforms = engine.registry().platforms();
    let tenant_ctx = Arc::new(make_tenant_ctx(
        "dev-viewer".into(),
        Some("demo".into()),
        Some("viewer".into()),
    ));
    let oauth_client = env::var("OAUTH_BASE_URL")
        .ok()
        .and_then(|_| OauthClient::from_env(reqwest::Client::new()).ok())
        .map(Arc::new);
    if oauth_client.is_none() {
        info!("OAUTH_BASE_URL not set or invalid; OAuth preview disabled");
    }
    let state = AppState {
        engine: Arc::new(engine),
        fixtures_dir: Arc::new(fixtures_dir),
        platforms,
        tenant_ctx,
        oauth_client,
    };

    let app = Router::new()
        .route("/", get(index))
        .route("/healthz", get(healthz))
        .route("/fixtures", get(list_fixtures))
        .route("/fixtures/:name", get(load_fixture))
        .route("/render", post(render_card))
        .with_state(state);

    info!(addr = %opts.listen, "dev viewer listening");

    let listener = tokio::net::TcpListener::bind(opts.listen).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

async fn index() -> impl IntoResponse {
    Html(INDEX_HTML)
}

async fn healthz() -> StatusCode {
    StatusCode::OK
}

async fn list_fixtures(
    State(state): State<AppState>,
) -> Result<Json<Vec<String>>, (StatusCode, String)> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(&*state.fixtures_dir).map_err(internal_error)? {
        let entry = entry.map_err(internal_error)?;
        if entry.file_type().map_err(internal_error)?.is_file()
            && let Some(name) = entry.file_name().to_str()
            && name.ends_with(".json")
        {
            names.push(name.to_string());
        }
    }
    names.sort();
    Ok(Json(names))
}

async fn load_fixture(
    State(state): State<AppState>,
    AxumPath(name): AxumPath<String>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let file_name =
        sanitize_name(&name).ok_or((StatusCode::BAD_REQUEST, "invalid fixture name".into()))?;
    let path = state.fixtures_dir.join(file_name);
    if !path.starts_with(&*state.fixtures_dir) {
        return Err((StatusCode::BAD_REQUEST, "fixture outside directory".into()));
    }
    let data = std::fs::read_to_string(&path).map_err(internal_error)?;
    let value: Value =
        serde_json::from_str(&data).map_err(|err| (StatusCode::BAD_REQUEST, err.to_string()))?;
    Ok(Json(value))
}

async fn render_card(
    State(state): State<AppState>,
    Json(request): Json<RenderRequest>,
) -> Result<Json<RenderResponse>, (StatusCode, String)> {
    let engine = state.engine.clone();
    if let Some(adaptive) = request.card.get("adaptive")
        && let Err(err) = validate_ac_json(adaptive)
    {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("adaptive card invalid: {err}"),
        ));
    }

    let mut card: MessageCard = serde_json::from_value(request.card).map_err(|err| {
        (
            StatusCode::BAD_REQUEST,
            format!("invalid MessageCard: {err}"),
        )
    })?;

    if matches!(card.kind, MessageCardKind::Oauth) {
        if let Some(client) = state.oauth_client.as_ref() {
            if let Err(err) =
                ensure_oauth_start_url(&mut card, &state.tenant_ctx, client, None).await
            {
                warn!(error = %err, "failed to build OAuth start URL");
            }
        }
    }

    let spec = engine
        .render_spec(&card)
        .map_err(|err| (StatusCode::BAD_REQUEST, format!("render_spec error: {err}")))?;

    let (ir_spec, auth_spec) = match &spec {
        RenderSpec::Card(ir) => (Some(ir.clone()), None),
        RenderSpec::Auth(auth) => (None, Some(auth.clone())),
    };

    let mut platforms = BTreeMap::new();
    for platform in &state.platforms {
        if let Some(snapshot) = engine.render_snapshot(platform, &spec) {
            let RenderSnapshot {
                output,
                ir,
                tier,
                target_tier,
                downgraded,
            } = snapshot;
            let (warnings, meta) = if let Some(ir) = ir {
                (ir.meta.warnings.clone(), ir.meta)
            } else {
                (output.warnings.clone(), Meta::default())
            };
            platforms.insert(
                platform.clone(),
                PlatformPreview {
                    payload: output.payload,
                    warnings,
                    tier,
                    target_tier,
                    downgraded,
                    used_modal: output.used_modal,
                    limit_exceeded: output.limit_exceeded,
                    sanitized_count: output.sanitized_count,
                    url_blocked_count: output.url_blocked_count,
                    meta,
                },
            );
        }
    }

    Ok(Json(RenderResponse {
        intent: spec.intent(),
        ir: ir_spec,
        auth: auth_spec,
        platforms,
    }))
}

fn sanitize_name(input: &str) -> Option<&str> {
    if input.contains('/') || input.contains('\\') || input.contains("..") {
        return None;
    }
    Some(input)
}

fn internal_error<E: ToString>(err: E) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
}

#[derive(Deserialize)]
struct RenderRequest {
    card: Value,
}

#[derive(Serialize)]
struct RenderResponse {
    intent: RenderIntent,
    ir: Option<MessageCardIr>,
    auth: Option<AuthRenderSpec>,
    platforms: BTreeMap<String, PlatformPreview>,
}

#[derive(Serialize)]
struct PlatformPreview {
    payload: Value,
    warnings: Vec<String>,
    tier: Tier,
    target_tier: Tier,
    downgraded: bool,
    used_modal: bool,
    limit_exceeded: bool,
    sanitized_count: usize,
    url_blocked_count: usize,
    meta: Meta,
}

const INDEX_HTML: &str = include_str!("index.html");
