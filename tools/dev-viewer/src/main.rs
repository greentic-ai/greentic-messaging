use std::collections::BTreeMap;
use std::env;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::{Path as AxumPath, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use axum::{Json, Router};
use clap::Parser;
use gsm_core::messaging_card::adaptive::validator::validate_ac_json;
use gsm_core::messaging_card::ir::{MessageCardIr, Meta};
use gsm_core::messaging_card::{
    AuthRenderSpec, MessageCard, MessageCardEngine, MessageCardKind, RenderIntent, RenderSnapshot,
    RenderSpec, ensure_oauth_start_url,
};
use gsm_core::oauth::{OauthClient, ReqwestTransport};
use gsm_core::{TenantCtx, Tier, make_tenant_ctx};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio::signal;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

mod pack_io;
mod provider_ext;
mod provider_registry;

use provider_registry::{ProviderInfo, ProviderRegistry};

#[derive(Parser, Debug)]
#[command(author, version, about = "Preview MessageCards across platforms", long_about = None)]
struct Opts {
    /// Address to bind the dev viewer (e.g. 127.0.0.1:7878)
    #[arg(long, default_value = "127.0.0.1:7878")]
    listen: SocketAddr,

    /// Directory containing MessageCard fixtures
    #[arg(long, default_value = "libs/core/tests/fixtures/cards")]
    fixtures: PathBuf,

    /// Additional provider packs to load (.gtpack files)
    #[arg(long, value_name = "PATH")]
    provider_pack: Vec<PathBuf>,

    /// Directory containing .gtpack bundles to enumerate
    #[arg(long, value_name = "DIR")]
    packs_dir: Option<PathBuf>,
}

#[derive(Clone)]
#[allow(dead_code)]
struct AppState {
    engine: Arc<MessageCardEngine>,
    fixtures_dir: Arc<PathBuf>,
    platforms: Vec<String>,
    tenant_ctx: Arc<TenantCtx>,
    oauth_client: Option<Arc<OauthClient<ReqwestTransport>>>,
    provider_registry: Arc<ProviderRegistry>,
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
        anyhow::bail!("fixtures directory {fixtures_dir:?} does not exist");
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
        .and_then(|raw| Url::parse(raw.trim()).ok())
        .map(|base_url| OauthClient::new(reqwest::Client::new(), base_url))
        .map(Arc::new);
    if oauth_client.is_none() {
        info!("OAUTH_BASE_URL not set or invalid; OAuth preview disabled");
    }

    verify_pack_paths(&opts.provider_pack)?;
    let packs_dir = opts.packs_dir.clone().or_else(default_provider_packs_dir);
    if let Some(dir) = packs_dir.as_ref() {
        info!(packs_dir = %dir.display(), "loading provider packs");
    }
    let packs_dir = verify_packs_dir(packs_dir.as_deref())?;
    let pack_paths = pack_io::discover_packs(&opts.provider_pack, packs_dir.as_deref())?;
    let provider_registry = if pack_paths.is_empty() {
        ProviderRegistry::empty()
    } else {
        let registry = ProviderRegistry::from_pack_paths(&pack_paths)?;
        info!(
            count = registry.entries().len(),
            "loaded messaging providers from packs"
        );
        registry
    };
    let state = AppState {
        engine: Arc::new(engine),
        fixtures_dir: Arc::new(fixtures_dir),
        platforms,
        tenant_ctx,
        oauth_client,
        provider_registry: Arc::new(provider_registry),
    };

    let app = Router::new()
        .route("/", get(index))
        .route("/healthz", get(healthz))
        .route("/fixtures", get(list_fixtures))
        .route("/fixtures/{name}", get(load_fixture))
        .route("/render", post(render_card))
        .route("/providers", get(list_providers))
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

    let mut card_value = wrap_adaptive_payload(request.card.clone());
    ensure_adaptive_card_versions(&mut card_value);
    ensure_column_types(&mut card_value);
    flatten_column_sets(&mut card_value);
    normalize_card_actions(&mut card_value);

    let mut card: MessageCard = serde_json::from_value(card_value).map_err(|err| {
        (
            StatusCode::BAD_REQUEST,
            format!("invalid MessageCard: {err}"),
        )
    })?;

    if matches!(card.kind, MessageCardKind::Oauth)
        && let Some(client) = state.oauth_client.as_ref()
        && let Err(err) = ensure_oauth_start_url(&mut card, &state.tenant_ctx, client, None).await
    {
        warn!(error = %err, "failed to build OAuth start URL");
    }

    let spec = engine
        .render_spec(&card)
        .map_err(|err| (StatusCode::BAD_REQUEST, format!("render_spec error: {err}")))?;

    let (ir_spec, auth_spec) = match &spec {
        RenderSpec::Card(ir) => (Some((**ir).clone()), None),
        RenderSpec::Auth(auth) => (None, Some(auth.clone())),
    };

    let selected_provider =
        resolve_selected_provider(&state.provider_registry, request.provider_id.as_deref())?;

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
        selected_provider: selected_provider.as_ref().map(selected_provider_response),
    }))
}

async fn list_providers(State(state): State<AppState>) -> Json<Vec<ProviderInfo>> {
    Json(state.provider_registry.entries().to_vec())
}

fn resolve_selected_provider(
    registry: &ProviderRegistry,
    provider_id: Option<&str>,
) -> Result<Option<ProviderInfo>, (StatusCode, String)> {
    if registry.is_empty() {
        return Ok(None);
    }
    let provider_id = provider_id.ok_or((
        StatusCode::BAD_REQUEST,
        "provider_id is required when provider packs are loaded".into(),
    ))?;
    registry.get(provider_id).cloned().map(Some).ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            format!("provider {provider_id} not found"),
        )
    })
}

fn selected_provider_response(info: &ProviderInfo) -> SelectedProviderResponse {
    SelectedProviderResponse {
        provider_id: info.id.clone(),
        runtime_component: info.runtime.component_ref.clone(),
        runtime_world: info.runtime.world.clone(),
        pack_path: info.pack_spec.display().to_string(),
        pack_root: info.pack_root.display().to_string(),
    }
}

fn sanitize_name(input: &str) -> Option<&str> {
    if input.contains('/') || input.contains('\\') || input.contains("..") {
        return None;
    }
    Some(input)
}

fn verify_pack_paths(paths: &[PathBuf]) -> anyhow::Result<()> {
    for path in paths {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if !canonical.exists() {
            anyhow::bail!("provider pack {:?} does not exist", path);
        }
        if canonical.is_dir() {
            continue;
        }
        if canonical
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("gtpack"))
            .unwrap_or(false)
        {
            continue;
        }
        anyhow::bail!(
            "provider pack {:?} must be a .gtpack file or directory",
            path
        );
    }
    Ok(())
}

fn verify_packs_dir(dir: Option<&Path>) -> anyhow::Result<Option<PathBuf>> {
    if let Some(path) = dir {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if !canonical.exists() {
            anyhow::bail!("packs dir {:?} does not exist", path);
        }
        if !canonical.is_dir() {
            anyhow::bail!("packs dir {:?} is not a directory", path);
        }
        Ok(Some(canonical))
    } else {
        Ok(None)
    }
}

fn default_provider_packs_dir() -> Option<PathBuf> {
    const FALLBACKS: &[&str] = &[
        "../greentic-messaging-providers/dist/packs",
        "../../greentic-messaging-providers/dist/packs",
    ];
    for relative in FALLBACKS {
        let candidate = Path::new(relative);
        if candidate.is_dir() {
            return candidate.canonicalize().ok();
        }
    }
    None
}

const DEFAULT_ADAPTIVE_CARD_VERSION: &str = "1.6";

fn wrap_adaptive_payload(card: Value) -> Value {
    match card {
        Value::Object(map) => wrap_adaptive_map(map),
        other => other,
    }
}

fn wrap_adaptive_map(map: Map<String, Value>) -> Value {
    if map.contains_key("adaptive") || !is_adaptive_card(&map) {
        Value::Object(map)
    } else {
        let mut wrapper = Map::new();
        wrapper.insert("adaptive".into(), Value::Object(map));
        Value::Object(wrapper)
    }
}

fn is_adaptive_card(map: &Map<String, Value>) -> bool {
    matches!(
        map.get("type").and_then(|value| value.as_str()),
        Some("AdaptiveCard")
    ) && (map.contains_key("body") || map.contains_key("$schema"))
}

fn normalize_card_actions(card: &mut Value) {
    fn normalized_action_type(action_type: &str) -> Option<&'static str> {
        match action_type {
            "open_url" | "Action.OpenUrl" => Some("open_url"),
            "post_back" | "Action.Submit" | "Action.Execute" => Some("post_back"),
            _ => None,
        }
    }

    if let Some(actions) = card.get_mut("actions").and_then(Value::as_array_mut) {
        let mut idx = 0;
        while idx < actions.len() {
            if let Some(obj) = actions[idx].as_object_mut()
                && let Some(action_type) = obj
                    .get("type")
                    .and_then(|value| value.as_str())
                    .and_then(normalized_action_type)
            {
                obj.insert("type".into(), Value::String(action_type.to_string()));
                idx += 1;
                continue;
            }
            actions.remove(idx);
        }
    }
}

fn ensure_adaptive_card_versions(card: &mut Value) {
    fn ensure(value: &mut Value) {
        match value {
            Value::Object(map) => {
                if map.get("type").and_then(|v| v.as_str()) == Some("AdaptiveCard")
                    && !map.contains_key("version")
                {
                    map.insert(
                        "version".into(),
                        Value::String(DEFAULT_ADAPTIVE_CARD_VERSION.into()),
                    );
                }
                for entry in map.values_mut() {
                    ensure(entry);
                }
            }
            Value::Array(elements) => {
                for element in elements {
                    ensure(element);
                }
            }
            _ => {}
        }
    }

    ensure(card);
}

fn ensure_column_types(card: &mut Value) {
    fn ensure(value: &mut Value) {
        match value {
            Value::Object(map) => {
                if map.get("type").and_then(|v| v.as_str()) == Some("ColumnSet")
                    && let Some(Value::Array(columns)) = map.get_mut("columns")
                {
                    for column in columns {
                        if let Value::Object(column_map) = column {
                            column_map
                                .entry("type")
                                .or_insert_with(|| Value::String("Column".into()));
                        }
                    }
                }
                for entry in map.values_mut() {
                    ensure(entry);
                }
            }
            Value::Array(elements) => {
                for element in elements {
                    ensure(element);
                }
            }
            _ => {}
        }
    }

    ensure(card);
}

fn flatten_column_sets(card: &mut Value) {
    match card {
        Value::Object(map) => {
            if let Some(body) = map.get_mut("body").and_then(Value::as_array_mut) {
                flatten_body(body);
            }
            for value in map.values_mut() {
                flatten_column_sets(value);
            }
        }
        Value::Array(array) => {
            flatten_body(array);
        }
        _ => {}
    }
}

fn flatten_body(body: &mut Vec<Value>) {
    let mut idx = 0;
    while idx < body.len() {
        if let Some(obj) = body[idx].as_object()
            && obj.get("type").and_then(|v| v.as_str()) == Some("ColumnSet")
        {
            let replacements = collect_column_items(obj);
            body.splice(idx..idx + 1, replacements);
            continue;
        }
        flatten_column_sets(&mut body[idx]);
        idx += 1;
    }
}

fn collect_column_items(column_set: &Map<String, Value>) -> Vec<Value> {
    let mut result = Vec::new();
    if let Some(columns) = column_set.get("columns").and_then(|v| v.as_array()) {
        for column in columns {
            if let Some(items) = column
                .as_object()
                .and_then(|col_map| col_map.get("items"))
                .and_then(|v| v.as_array())
            {
                for item in items {
                    let mut item_clone = item.clone();
                    flatten_column_sets(&mut item_clone);
                    result.push(item_clone);
                }
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_ADAPTIVE_CARD_VERSION, ensure_adaptive_card_versions, ensure_column_types,
        flatten_column_sets, normalize_card_actions, sanitize_name, wrap_adaptive_payload,
    };
    use serde_json::json;

    #[test]
    fn sanitize_name_accepts_plain_filename() {
        assert_eq!(sanitize_name("card.json"), Some("card.json"));
    }

    #[test]
    fn sanitize_name_rejects_paths_with_slashes() {
        assert!(sanitize_name("foo/bar.json").is_none());
        assert!(sanitize_name("foo\\bar.json").is_none());
    }

    #[test]
    fn sanitize_name_rejects_path_traversal() {
        assert!(sanitize_name("..").is_none());
        assert!(sanitize_name("foo..bar").is_none());
        assert!(sanitize_name("foo/..").is_none());
    }

    #[test]
    fn normalize_card_actions_keeps_supported_types() {
        let mut card = json!({
            "actions": [
                { "type": "Action.OpenUrl", "title": "Docs", "url": "https://example.com" },
                { "type": "Action.Submit", "title": "Submit", "data": {} }
            ]
        });
        normalize_card_actions(&mut card);
        let actions = card["actions"].as_array().unwrap();
        assert_eq!(actions.len(), 2);
        assert_eq!(actions[0]["type"], "open_url");
        assert_eq!(actions[1]["type"], "post_back");
    }

    #[test]
    fn normalize_card_actions_drops_unsupported_types() {
        let mut card = json!({
            "actions": [
                { "type": "Action.ShowCard", "title": "More" },
                { "type": "open_url", "title": "Docs", "url": "https://example.com" }
            ]
        });
        normalize_card_actions(&mut card);
        let actions = card["actions"].as_array().unwrap();
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0]["type"], "open_url");
    }

    #[test]
    fn wrap_adaptive_payload_wraps_root_adaptive_card() {
        let adaptive = json!({
            "type": "AdaptiveCard",
            "$schema": "http://adaptivecards.io/schemas/adaptive-card.json",
            "version": "1.6",
            "body": [{ "type": "TextBlock", "text": "Nested" }]
        });
        let wrapped = wrap_adaptive_payload(adaptive.clone());
        let inner = wrapped["adaptive"].clone();
        assert_eq!(inner, adaptive);
        assert!(wrapped.get("type").is_none());
    }

    #[test]
    fn wrap_adaptive_payload_is_idempotent_for_wrapped_cards() {
        let card = json!({
            "adaptive": {
                "type": "AdaptiveCard",
                "body": []
            },
            "text": "hi"
        });
        let wrapped = wrap_adaptive_payload(card.clone());
        assert_eq!(wrapped, card);
    }

    #[test]
    fn wrap_adaptive_payload_leaves_plain_cards_alone() {
        let card = json!({
            "text": "hello",
            "actions": [{ "type": "open_url", "title": "Docs", "url": "https://example.com" }]
        });
        let wrapped = wrap_adaptive_payload(card.clone());
        assert_eq!(wrapped, card);
    }

    #[test]
    fn ensure_adaptive_card_versions_inserts_missing_version() {
        let mut card = json!({
            "type": "AdaptiveCard",
            "body": [
                {
                    "type": "Action.ShowCard",
                    "card": {
                        "type": "AdaptiveCard",
                        "body": []
                    }
                }
            ]
        });
        ensure_adaptive_card_versions(&mut card);
        assert_eq!(card["version"], DEFAULT_ADAPTIVE_CARD_VERSION);
        assert_eq!(
            card["body"][0]["card"]["version"],
            DEFAULT_ADAPTIVE_CARD_VERSION
        );
    }

    #[test]
    fn ensure_adaptive_card_versions_preserves_existing_version() {
        let mut card = json!({
            "type": "AdaptiveCard",
            "version": "1.5",
            "actions": [
                {
                    "type": "Action.ShowCard",
                    "card": {
                        "type": "AdaptiveCard",
                        "version": "1.2",
                        "body": []
                    }
                }
            ]
        });
        ensure_adaptive_card_versions(&mut card);
        assert_eq!(card["version"], "1.5");
        assert_eq!(card["actions"][0]["card"]["version"], "1.2");
    }

    #[test]
    fn ensure_column_types_inserts_missing_column_type() {
        let mut card = json!({
            "type": "AdaptiveCard",
            "body": [
                {
                    "type": "ColumnSet",
                    "columns": [
                        {
                            "width": "auto",
                            "items": [{ "type": "TextBlock", "text": "Hello" }]
                        }
                    ]
                }
            ]
        });
        ensure_column_types(&mut card);
        assert_eq!(card["body"][0]["columns"][0]["type"], "Column");
    }

    #[test]
    fn ensure_column_types_preserves_existing_column_type() {
        let mut card = json!({
            "type": "AdaptiveCard",
            "body": [
                {
                    "type": "ColumnSet",
                    "columns": [
                        {
                            "type": "Column",
                            "width": "stretch",
                            "items": []
                        }
                    ]
                }
            ]
        });
        ensure_column_types(&mut card);
        assert_eq!(card["body"][0]["columns"][0]["type"], "Column");
    }

    #[test]
    fn flatten_column_sets_replaces_columnset_with_items() {
        let mut card = json!({
            "type": "AdaptiveCard",
            "body": [
                {
                    "type": "ColumnSet",
                    "columns": [
                        {
                            "type": "Column",
                            "items": [
                                { "type": "TextBlock", "text": "A" }
                            ]
                        },
                        {
                            "type": "Column",
                            "items": [
                                { "type": "TextBlock", "text": "B" }
                            ]
                        }
                    ]
                }
            ]
        });
        flatten_column_sets(&mut card);
        let body = card["body"].as_array().unwrap();
        assert_eq!(body.len(), 2);
        assert_eq!(body[0]["text"], "A");
        assert_eq!(body[1]["text"], "B");
    }
}

fn internal_error<E: ToString>(err: E) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
}

#[derive(Deserialize)]
struct RenderRequest {
    card: Value,
    provider_id: Option<String>,
}

#[derive(Serialize)]
struct RenderResponse {
    intent: RenderIntent,
    ir: Option<MessageCardIr>,
    auth: Option<AuthRenderSpec>,
    platforms: BTreeMap<String, PlatformPreview>,
    selected_provider: Option<SelectedProviderResponse>,
}

#[derive(Serialize)]
struct SelectedProviderResponse {
    provider_id: String,
    runtime_component: String,
    runtime_world: String,
    pack_path: String,
    pack_root: String,
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
