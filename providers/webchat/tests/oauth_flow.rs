use std::{collections::HashMap, sync::Arc};

use axum::{body::to_bytes, extract::State, response::IntoResponse};
use greentic_messaging_providers_webchat::{
    config::OAuthProviderConfig,
    conversation::memory_store,
    directline_client::{DirectLineError, MockDirectLineApi},
    http::{AppState, DirectLinePoster, SharedDirectLinePoster},
    oauth::{self, CLOSE_WINDOW_HTML, CallbackQuery, GreenticOauthClient, StartQuery},
    session::{MemorySessionStore, SharedSessionStore, WebchatSession, WebchatSessionStore},
};
use greentic_types::{EnvId, TenantCtx, TenantId};
use reqwest::Client;
use serde_json::Value;
use tokio::sync::Mutex;

fn tenant_ctx() -> TenantCtx {
    TenantCtx::new(EnvId::from("dev"), TenantId::from("acme"))
}

fn config_with_oauth(
    base: &str,
    entries: &[(&str, &str)],
) -> greentic_messaging_providers_webchat::config::Config {
    let map: HashMap<String, String> = entries
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();
    let map = Arc::new(map);
    greentic_messaging_providers_webchat::config::Config::with_base_url(base).with_oauth_lookup({
        let map = Arc::clone(&map);
        move |key| map.get(key).cloned()
    })
}

#[tokio::test]
async fn oauth_start_redirects_to_authorize() {
    let direct_line = Arc::new(MockDirectLineApi::default());
    let client = Client::builder().build().unwrap();
    let store = Arc::new(MemorySessionStore::default());
    let sessions: SharedSessionStore = store.clone();
    store
        .upsert(WebchatSession::new(
            "conversation-123".to_string(),
            tenant_ctx(),
            "token-abc".to_string(),
        ))
        .await
        .unwrap();

    let state = AppState::new(
        config_with_oauth(
            "https://directline.test/v3/directline",
            &[
                (
                    "WEBCHAT_OAUTH_ISSUER__DEV__ACME",
                    "https://oauth.greentic.dev",
                ),
                ("WEBCHAT_OAUTH_CLIENT_ID__DEV__ACME", "client-xyz"),
                (
                    "WEBCHAT_OAUTH_REDIRECT_BASE__DEV__ACME",
                    "https://messaging.greentic.dev",
                ),
            ],
        ),
        direct_line,
        client,
    )
    .with_sessions(sessions);

    let response = oauth::start(
        State(state),
        axum::extract::Query(StartQuery {
            conversation_id: "conversation-123".into(),
            state: Some("bf-state-1".into()),
        }),
    )
    .await
    .expect("start ok")
    .into_response();

    assert_eq!(
        response.status(),
        axum::http::StatusCode::TEMPORARY_REDIRECT
    );
    let location = response
        .headers()
        .get(axum::http::header::LOCATION)
        .expect("location header")
        .to_str()
        .unwrap();
    assert!(location.starts_with("https://oauth.greentic.dev/authorize"));
    assert!(
        location.contains("client_id=client-xyz"),
        "location missing client id {location}"
    );
    assert!(
        location.contains("redirect_uri=https%3A%2F%2Fmessaging.greentic.dev%2Fwebchat%2Foauth%2Fcallback%3FconversationId%3Dconversation-123"),
        "redirect uri mismatch {location}"
    );
    assert!(location.contains("state=bf-state-1"));
}

#[tokio::test]
async fn oauth_callback_exchanges_code_and_posts_handle() {
    let direct_line = Arc::new(MockDirectLineApi::default());
    let client = Client::builder().build().unwrap();
    let store = Arc::new(MemorySessionStore::default());
    store
        .upsert(WebchatSession::new(
            "conversation-999".to_string(),
            tenant_ctx(),
            "token-directline".to_string(),
        ))
        .await
        .unwrap();
    let sessions: SharedSessionStore = store.clone();

    let poster = Arc::new(MockPoster::default());
    let oauth_client = Arc::new(MockOauthClient::new("opaque-456"));
    let conversations = memory_store();

    let poster_arc: SharedDirectLinePoster = poster.clone();
    let oauth_arc: Arc<dyn GreenticOauthClient> = oauth_client.clone();

    let state = AppState::new(
        config_with_oauth(
            "https://directline.test/v3/directline",
            &[
                (
                    "WEBCHAT_OAUTH_ISSUER__DEV__ACME",
                    "https://oauth.greentic.dev",
                ),
                ("WEBCHAT_OAUTH_CLIENT_ID__DEV__ACME", "client-xyz"),
                (
                    "WEBCHAT_OAUTH_REDIRECT_BASE__DEV__ACME",
                    "https://messaging.greentic.dev",
                ),
            ],
        ),
        direct_line,
        client,
    )
    .with_sessions(sessions)
    .with_activity_poster(poster_arc)
    .with_oauth_client(oauth_arc)
    .with_conversations(conversations.clone());

    let response = oauth::callback(
        State(state),
        axum::extract::Query(CallbackQuery {
            conversation_id: "conversation-999".into(),
            code: Some("auth-code-1".into()),
            state: Some("bf-state-9".into()),
            error: None,
        }),
    )
    .await
    .expect("callback ok")
    .into_response();

    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(body, CLOSE_WINDOW_HTML.as_bytes());

    let calls = poster.calls.lock().await;
    assert!(calls.is_empty(), "should not call remote Direct Line");

    let page = conversations
        .activities("conversation-999", None)
        .await
        .expect("conversation activities");
    assert_eq!(page.activities.len(), 1);
    let stored = &page.activities[0].activity;
    assert_eq!(stored.text.as_deref(), Some("You're signed in."));
    assert_eq!(
        stored
            .channel_data
            .as_ref()
            .and_then(|value| value.get("oauth_token_handle"))
            .and_then(Value::as_str),
        Some("opaque-456")
    );
    assert_eq!(
        stored.from.as_ref().map(|from| from.id.as_str()),
        Some("bot")
    );

    let exchanges = oauth_client.calls.lock().await;
    assert_eq!(exchanges.len(), 1);
    assert_eq!(exchanges[0].code, "auth-code-1");
    assert_eq!(
        exchanges[0].redirect_uri,
        "https://messaging.greentic.dev/webchat/oauth/callback?conversationId=conversation-999"
    );
}

#[derive(Default)]
struct MockPoster {
    calls: Mutex<Vec<RecordedActivity>>,
}

#[allow(dead_code)]
struct RecordedActivity {
    conversation_id: String,
    bearer_token: String,
    activity: Value,
}

#[async_trait::async_trait]
impl DirectLinePoster for MockPoster {
    async fn post_activity(
        &self,
        _base_url: &str,
        conversation_id: &str,
        bearer_token: &str,
        activity: Value,
    ) -> Result<(), DirectLineError> {
        self.calls.lock().await.push(RecordedActivity {
            conversation_id: conversation_id.to_string(),
            bearer_token: bearer_token.to_string(),
            activity,
        });
        Ok(())
    }
}

struct MockOauthClient {
    handle: String,
    calls: Mutex<Vec<ExchangeCall>>,
}

struct ExchangeCall {
    code: String,
    redirect_uri: String,
}

impl MockOauthClient {
    fn new(handle: &str) -> Self {
        Self {
            handle: handle.to_string(),
            calls: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait::async_trait]
impl GreenticOauthClient for MockOauthClient {
    async fn exchange_code(
        &self,
        _: &TenantCtx,
        _: &OAuthProviderConfig,
        code: &str,
        redirect_uri: &str,
    ) -> Result<String, anyhow::Error> {
        self.calls.lock().await.push(ExchangeCall {
            code: code.to_string(),
            redirect_uri: redirect_uri.to_string(),
        });
        Ok(self.handle.clone())
    }
}
