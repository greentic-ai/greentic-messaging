use async_trait::async_trait;
use axum::{
    extract::{Query, State},
    response::{IntoResponse, Redirect},
    routing::get,
    Json, Router,
};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use gsm_core::platforms::slack::workspace::SlackWorkspace;
use gsm_core::{
    make_tenant_ctx, slack_workspace_secret, DefaultResolver, NodeResult, SecretsResolver,
    TenantCtx,
};
use rand::{distributions::Alphanumeric, Rng};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;
use tracing::instrument;

#[derive(Clone)]
struct AppState {
    client: reqwest::Client,
    client_id: String,
    client_secret: String,
    redirect_uri: String,
    scope: String,
    user_scope: Option<String>,
    api_base: String,
    store: Arc<dyn WorkspaceStore>,
}

#[async_trait]
trait WorkspaceStore: Send + Sync {
    async fn put_workspace(&self, ctx: &TenantCtx, workspace: &SlackWorkspace) -> NodeResult<()>;
    #[cfg_attr(not(test), allow(dead_code))]
    async fn get_workspace(
        &self,
        ctx: &TenantCtx,
        workspace_id: &str,
    ) -> NodeResult<Option<SlackWorkspace>>;
}

#[async_trait]
impl WorkspaceStore for DefaultResolver {
    async fn put_workspace(&self, ctx: &TenantCtx, workspace: &SlackWorkspace) -> NodeResult<()> {
        let path = slack_workspace_secret(ctx, &workspace.workspace_id);
        self.put_json(&path, ctx, workspace).await
    }

    async fn get_workspace(
        &self,
        ctx: &TenantCtx,
        workspace_id: &str,
    ) -> NodeResult<Option<SlackWorkspace>> {
        let path = slack_workspace_secret(ctx, workspace_id);
        self.get_json(&path, ctx).await
    }
}

#[derive(Deserialize)]
struct InstallQuery {
    tenant: String,
    #[serde(default)]
    team: Option<String>,
}

#[derive(Deserialize)]
struct CallbackQuery {
    code: String,
    state: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let client_id = std::env::var("SLACK_CLIENT_ID")?;
    let client_secret = std::env::var("SLACK_CLIENT_SECRET")?;
    let redirect_uri = std::env::var("SLACK_REDIRECT_URI")?;
    let scope = std::env::var("SLACK_SCOPES").unwrap_or_else(|_| "commands,chat:write".into());
    let user_scope = std::env::var("SLACK_USER_SCOPES")
        .ok()
        .filter(|s| !s.is_empty());
    let api_base =
        std::env::var("SLACK_API_BASE").unwrap_or_else(|_| "https://slack.com/api".into());

    let store: Arc<dyn WorkspaceStore> = Arc::new(DefaultResolver::new().await?);
    let state = Arc::new(AppState {
        client: reqwest::Client::new(),
        client_id,
        client_secret,
        redirect_uri,
        scope,
        user_scope,
        api_base,
        store,
    });

    let app = Router::new()
        .route("/slack/install", get(install))
        .route("/slack/callback", get(callback))
        .with_state(state);

    let addr: std::net::SocketAddr = std::env::var("BIND")
        .unwrap_or_else(|_| "0.0.0.0:8091".into())
        .parse()?;
    tracing::info!(%addr, "slack oauth handler listening");
    axum::serve(tokio::net::TcpListener::bind(addr).await?, app).await?;
    Ok(())
}

async fn install(
    State(state): State<Arc<AppState>>,
    Query(query): Query<InstallQuery>,
) -> impl IntoResponse {
    let tenant = query.tenant;
    let team = query.team.unwrap_or_else(|| "default".into());
    let nonce: String = rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(12)
        .map(char::from)
        .collect();
    let payload = format!("{}|{}|{}", tenant, team, nonce);
    let encoded_state = URL_SAFE_NO_PAD.encode(payload);

    let mut authorize = format!(
        "https://slack.com/oauth/v2/authorize?client_id={}&scope={}&state={}&redirect_uri={}",
        state.client_id,
        urlencoding::encode(&state.scope),
        encoded_state,
        urlencoding::encode(&state.redirect_uri)
    );
    if let Some(user_scope) = &state.user_scope {
        if !user_scope.is_empty() {
            authorize.push_str("&user_scope=");
            authorize.push_str(&urlencoding::encode(user_scope));
        }
    }

    Redirect::permanent(&authorize)
}

async fn callback(
    State(state): State<Arc<AppState>>,
    Query(query): Query<CallbackQuery>,
) -> impl IntoResponse {
    match handle_callback(state, query).await {
        Ok(value) => (axum::http::StatusCode::OK, Json(value)).into_response(),
        Err(err) => {
            tracing::error!(error = %err, "slack oauth callback failed");
            (
                axum::http::StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "ok": false,
                    "error": format!("{}", err),
                })),
            )
                .into_response()
        }
    }
}

#[instrument(skip_all)]
async fn handle_callback(state: Arc<AppState>, query: CallbackQuery) -> anyhow::Result<Value> {
    let (tenant, team) = decode_state(&query.state)?;
    let ctx = make_tenant_ctx(tenant, team.clone(), None);
    let workspace = exchange_code(&state, &query.code).await?;
    store_workspace(&state, &ctx, &workspace).await?;

    Ok(serde_json::json!({
        "ok": true,
        "workspace_id": workspace.workspace_id,
        "team": team.unwrap_or_else(|| "default".into()),
    }))
}

async fn store_workspace(
    state: &AppState,
    ctx: &TenantCtx,
    workspace: &SlackWorkspace,
) -> anyhow::Result<()> {
    state
        .store
        .put_workspace(ctx, workspace)
        .await
        .map_err(|err| anyhow::anyhow!("failed to store workspace: {err}"))?;
    Ok(())
}

async fn exchange_code(state: &AppState, code: &str) -> anyhow::Result<SlackWorkspace> {
    if state.api_base.starts_with("mock://") {
        return Ok(SlackWorkspace {
            workspace_id: "T123".into(),
            bot_token: "xoxb-mock".into(),
            enterprise_id: None,
        });
    }

    let url = format!("{}/oauth.v2.access", state.api_base.trim_end_matches('/'));
    let params = [
        ("client_id", state.client_id.as_str()),
        ("client_secret", state.client_secret.as_str()),
        ("code", code),
        ("redirect_uri", state.redirect_uri.as_str()),
    ];
    let response = state
        .client
        .post(&url)
        .form(&params)
        .send()
        .await?
        .error_for_status()?;
    let body: SlackOauthResponse = response.json().await?;
    if !body.ok {
        return Err(anyhow::anyhow!(body
            .error
            .unwrap_or_else(|| "slack oauth failed".into())));
    }
    let team_id = body
        .team
        .and_then(|t| t.id)
        .ok_or_else(|| anyhow::anyhow!("missing team id in oauth response"))?;
    let bot_token = body
        .access_token
        .ok_or_else(|| anyhow::anyhow!("missing bot token in oauth response"))?;
    Ok(SlackWorkspace {
        workspace_id: team_id,
        bot_token,
        enterprise_id: body.enterprise.and_then(|e| e.id),
    })
}

fn decode_state(state: &str) -> anyhow::Result<(String, Option<String>)> {
    let decoded = URL_SAFE_NO_PAD
        .decode(state)
        .map_err(|err| anyhow::anyhow!("invalid state: {err}"))?;
    let raw = String::from_utf8(decoded)?;
    let mut parts = raw.splitn(3, '|');
    let tenant = parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("state missing tenant"))?
        .to_string();
    let team_raw = parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("state missing team"))?
        .to_string();
    let team = normalize_team(&team_raw);
    Ok((tenant, team))
}

fn normalize_team(team: &str) -> Option<String> {
    let trimmed = team.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("default") {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[derive(Debug, Deserialize)]
struct SlackOauthResponse {
    ok: bool,
    access_token: Option<String>,
    error: Option<String>,
    team: Option<TeamInfo>,
    enterprise: Option<EnterpriseInfo>,
}

#[derive(Debug, Deserialize)]
struct TeamInfo {
    id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct EnterpriseInfo {
    id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use tower::ServiceExt;

    #[derive(Default)]
    struct InMemoryStore {
        data: Mutex<HashMap<(String, Option<String>, String), SlackWorkspace>>,
    }

    #[async_trait]
    impl WorkspaceStore for InMemoryStore {
        async fn put_workspace(
            &self,
            ctx: &TenantCtx,
            workspace: &SlackWorkspace,
        ) -> NodeResult<()> {
            let mut guard = self.data.lock().unwrap();
            guard.insert(
                (
                    ctx.tenant.as_str().to_string(),
                    ctx.team.as_ref().map(|t| t.as_str().to_string()),
                    workspace.workspace_id.clone(),
                ),
                workspace.clone(),
            );
            Ok(())
        }

        async fn get_workspace(
            &self,
            ctx: &TenantCtx,
            workspace_id: &str,
        ) -> NodeResult<Option<SlackWorkspace>> {
            let guard = self.data.lock().unwrap();
            Ok(guard
                .get(&(
                    ctx.tenant.as_str().to_string(),
                    ctx.team.as_ref().map(|t| t.as_str().to_string()),
                    workspace_id.to_string(),
                ))
                .cloned())
        }
    }

    #[tokio::test]
    async fn install_redirects_with_state() {
        let store = Arc::new(InMemoryStore::default());
        let state = Arc::new(AppState {
            client: reqwest::Client::new(),
            client_id: "CID".into(),
            client_secret: "SECRET".into(),
            redirect_uri: "https://example.com/callback".into(),
            scope: "commands".into(),
            user_scope: None,
            api_base: "mock://slack".into(),
            store: store.clone(),
        });
        let app = Router::new()
            .route("/slack/install", get(install))
            .with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/slack/install?tenant=acme&team=support")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::PERMANENT_REDIRECT);
        let location = response
            .headers()
            .get(axum::http::header::LOCATION)
            .unwrap();
        let location = location.to_str().unwrap();
        assert!(location.contains("client_id=CID"));
        assert!(location.contains("state="));
    }

    #[tokio::test]
    async fn callback_persists_workspace() {
        std::env::set_var("GREENTIC_ENV", "test");
        let store = Arc::new(InMemoryStore::default());
        let state = Arc::new(AppState {
            client: reqwest::Client::new(),
            client_id: "CID".into(),
            client_secret: "SECRET".into(),
            redirect_uri: "https://example.com/callback".into(),
            scope: "commands".into(),
            user_scope: None,
            api_base: "mock://slack".into(),
            store: store.clone(),
        });
        let app = Router::new()
            .route("/slack/callback", get(callback))
            .with_state(state.clone());

        let state_token = URL_SAFE_NO_PAD.encode("acme|support|nonce");
        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/slack/callback?code=abc&state={state_token}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(status, StatusCode::OK, "response: {json}");
        assert!(json["ok"].as_bool().unwrap());

        let ctx = make_tenant_ctx("acme".into(), Some("support".into()), None);
        let stored = store
            .get_workspace(&ctx, "T123")
            .await
            .unwrap()
            .expect("stored");
        assert_eq!(stored.bot_token, "xoxb-mock");
    }
}
