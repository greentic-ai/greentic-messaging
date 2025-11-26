use std::sync::Arc;

use axum::{body::to_bytes, extract::State, response::IntoResponse};
use greentic_types::TenantCtx;
use gsm_core::platforms::webchat::{
    config::{Config, OAuthProviderConfig},
    conversation::memory_store,
    directline_client::{DirectLineError, MockDirectLineApi},
    http::{AppState, DirectLinePoster},
    oauth::{self, CLOSE_WINDOW_HTML, CallbackQuery, GreenticOauthClient, StartQuery},
    session::{MemorySessionStore, SharedSessionStore, WebchatSession, WebchatSessionStore},
};
use reqwest::Client;
use serde_json::Value;

#[path = "webchat_support.rs"]
mod support;

use support::{provider_with_secrets, signing_scope, tenant_ctx, tenant_scope};

#[tokio::test]
async fn oauth_start_redirects_to_authorize() {
    let direct_line = Arc::new(MockDirectLineApi::default());
    let client = Client::builder().build().unwrap();
    let store = Arc::new(MemorySessionStore::default());
    let sessions: SharedSessionStore = store.clone();
    store
        .upsert(WebchatSession::new(
            "conversation-123".to_string(),
            tenant_ctx("dev", "acme", None),
            "token-abc".to_string(),
        ))
        .await
        .unwrap();

    let oauth_scope = tenant_scope("dev", "acme", None);
    let secrets = [
        (
            &oauth_scope,
            "webchat_oauth",
            "issuer",
            "https://oauth.example.com",
        ),
        (&oauth_scope, "webchat_oauth", "client_id", "webchat-client"),
        (
            &oauth_scope,
            "webchat_oauth",
            "redirect_base",
            "https://webchat.example.com",
        ),
    ];
    let provider = provider_with_secrets(
        Config::with_base_url("https://directline.test/v3/directline"),
        signing_scope(),
        &secrets,
    );

    let state = AppState::new(provider.clone(), direct_line, client)
        .with_sessions(sessions)
        .with_activity_poster(Arc::new(NoopPoster))
        .with_oauth_client(Arc::new(StaticOauthClient));

    let response = oauth::start(
        State(state),
        axum::extract::Query(StartQuery {
            conversation_id: "conversation-123".into(),
            state: Some("xyz".into()),
        }),
    )
    .await
    .unwrap()
    .into_response();

    assert_eq!(
        response.status(),
        axum::http::StatusCode::TEMPORARY_REDIRECT
    );
    let location = response
        .headers()
        .get(axum::http::header::LOCATION)
        .unwrap();
    let url = reqwest::Url::parse(location.to_str().unwrap()).unwrap();
    assert_eq!(url.scheme(), "https");
    assert_eq!(url.host_str(), Some("oauth.example.com"));
    assert_eq!(url.path(), "/authorize");
    let params: std::collections::HashMap<_, _> = url.query_pairs().collect();
    assert_eq!(params["client_id"], "webchat-client");
    assert_eq!(params["state"], "xyz");
}

#[tokio::test]
async fn oauth_callback_exchanges_code_and_posts_handle() {
    let direct_line = Arc::new(MockDirectLineApi::default());
    let client = Client::builder().build().unwrap();
    let sessions: SharedSessionStore = Arc::new(MemorySessionStore::default());
    sessions
        .upsert(WebchatSession::new(
            "conversation-123".to_string(),
            tenant_ctx("dev", "acme", None),
            "token-abc".to_string(),
        ))
        .await
        .unwrap();

    let oauth_scope = tenant_scope("dev", "acme", None);
    let provider = provider_with_secrets(
        Config::with_base_url("https://directline.test/v3/directline"),
        signing_scope(),
        &[
            (
                &oauth_scope,
                "webchat_oauth",
                "issuer",
                "https://oauth.example.com",
            ),
            (&oauth_scope, "webchat_oauth", "client_id", "webchat-client"),
            (
                &oauth_scope,
                "webchat_oauth",
                "redirect_base",
                "https://webchat.example.com",
            ),
        ],
    );

    let conversations = memory_store();
    #[cfg(feature = "directline_standalone")]
    {
        conversations
            .create("conversation-123", tenant_ctx("dev", "acme", None))
            .await
            .unwrap();
    }

    let state = AppState::new(provider, direct_line, client)
        .with_sessions(Arc::clone(&sessions))
        .with_activity_poster(Arc::new(NoopPoster))
        .with_oauth_client(Arc::new(StaticOauthClient))
        .with_conversations(conversations.clone());

    let response = oauth::callback(
        State(state),
        axum::extract::Query(CallbackQuery {
            conversation_id: "conversation-123".into(),
            code: Some("auth-code-xyz".into()),
            state: None,
            error: None,
        }),
    )
    .await
    .unwrap()
    .into_response();

    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let body = to_bytes(response.into_body(), 1024).await.unwrap();
    assert_eq!(body, CLOSE_WINDOW_HTML);

    #[cfg(feature = "directline_standalone")]
    {
        let page = conversations
            .activities("conversation-123", None)
            .await
            .unwrap();
        let bot_activity = page.activities.back().unwrap();
        assert_eq!(
            bot_activity.activity.text.as_deref(),
            Some("You're signed in.")
        );
        assert_eq!(
            bot_activity.activity.channel_data.as_ref().unwrap()["oauth_token_handle"],
            "token-handle-789"
        );
        let watermark = bot_activity.watermark;
        sessions
            .update_watermark("conversation-123", Some((watermark + 1).to_string()))
            .await
            .unwrap();
    }
}

struct NoopPoster;

#[async_trait::async_trait]
impl DirectLinePoster for NoopPoster {
    async fn post_activity(
        &self,
        _base_url: &str,
        _conversation_id: &str,
        _bearer_token: &str,
        _activity: Value,
    ) -> Result<(), DirectLineError> {
        Ok(())
    }
}

#[derive(Clone)]
struct StaticOauthClient;

#[async_trait::async_trait]
impl GreenticOauthClient for StaticOauthClient {
    async fn exchange_code(
        &self,
        _tenant_ctx: &TenantCtx,
        _config: &OAuthProviderConfig,
        _code: &str,
        _redirect_uri: &str,
    ) -> Result<String, anyhow::Error> {
        Ok("token-handle-789".into())
    }
}
