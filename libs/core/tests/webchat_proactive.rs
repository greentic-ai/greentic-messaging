use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
};
use greentic_types::TenantCtx;
use gsm_core::platforms::webchat::{
    config::{Config, OAuthProviderConfig},
    conversation::memory_store,
    directline_client::{DirectLineError, MockDirectLineApi},
    error::WebChatError,
    http::{
        AdminPath, AdminPostActivityRequest, AppState, DirectLinePoster, SharedDirectLinePoster,
        admin_post_activity,
    },
    oauth::GreenticOauthClient,
    session::{MemorySessionStore, SharedSessionStore, WebchatSession, WebchatSessionStore},
};
use reqwest::Client;
use serde_json::{Value, json};
use tokio::sync::Mutex;

#[path = "webchat_support.rs"]
mod support;

use support::{provider_with_secrets, signing_scope, tenant_ctx, tenant_scope};

#[tokio::test]
async fn admin_posts_to_specific_conversation() {
    let direct_line = Arc::new(MockDirectLineApi::default());
    let client = Client::builder().build().unwrap();
    let store = Arc::new(MemorySessionStore::default());
    let sessions: SharedSessionStore = store.clone();
    store
        .upsert(WebchatSession::new(
            "conv-1".to_string(),
            tenant_ctx("dev", "acme", Some("support")),
            "token-1".to_string(),
        ))
        .await
        .unwrap();
    let conversations = memory_store();
    conversations
        .create("conv-1", tenant_ctx("dev", "acme", Some("support")))
        .await
        .unwrap();

    let poster = Arc::new(RecordingPoster::default());
    let scope = tenant_scope("dev", "acme", None);
    let provider = provider_with_secrets(
        Config::with_base_url("https://directline.test/v3/directline"),
        signing_scope(),
        &[(&scope, "webchat", "channel_token", "dl-secret")],
    );
    let state = AppState::new(provider, direct_line, client.clone())
        .with_sessions(sessions)
        .with_activity_poster(poster.clone() as SharedDirectLinePoster)
        .with_oauth_client(Arc::new(StubOauthClient))
        .with_conversations(conversations.clone());

    let Json(payload) = admin_post_activity(
        State(state),
        Path(AdminPath {
            env: "dev".to_string(),
            tenant: "acme".to_string(),
        }),
        Json(AdminPostActivityRequest {
            team: None,
            conversation_id: Some("conv-1".to_string()),
            activity: json!({
                "type": "message",
                "text": "hello proactive"
            }),
        }),
    )
    .await
    .unwrap();

    assert_eq!(payload.posted, 1);
    assert_eq!(payload.skipped, 0);

    let calls = poster.calls.lock().await;
    assert!(calls.is_empty(), "should not post via Direct Line");

    let page = conversations
        .activities("conv-1", None)
        .await
        .expect("conversation activities");
    assert_eq!(page.activities.len(), 1);
    let stored = &page.activities[0].activity;
    assert_eq!(stored.text.as_deref(), Some("hello proactive"));
    assert_eq!(
        stored
            .from
            .as_ref()
            .map(|from| (from.id.as_str(), from.role.as_deref())),
        Some(("bot", Some("bot")))
    );
    let session = store.get("conv-1").await.unwrap().unwrap();
    assert_eq!(session.watermark.as_deref(), Some("1"));
}

#[tokio::test]
async fn admin_broadcast_respects_proactive_flags() {
    let direct_line = Arc::new(MockDirectLineApi::default());
    let client = Client::builder().build().unwrap();
    let store = Arc::new(MemorySessionStore::default());
    let sessions: SharedSessionStore = store.clone();

    store
        .upsert(WebchatSession::new(
            "conv-allow".to_string(),
            tenant_ctx("DEV", "ACME", Some("Support")),
            "token-allow".to_string(),
        ))
        .await
        .unwrap();
    store
        .upsert(WebchatSession::new(
            "conv-blocked".to_string(),
            tenant_ctx("dev", "acme", Some("support")),
            "token-blocked".to_string(),
        ))
        .await
        .unwrap();
    store.set_proactive("conv-blocked", false).await.unwrap();
    store
        .upsert(WebchatSession::new(
            "conv-other".to_string(),
            tenant_ctx("dev", "acme", Some("other")),
            "token-other".to_string(),
        ))
        .await
        .unwrap();
    let conversations = memory_store();
    conversations
        .create("conv-allow", tenant_ctx("dev", "acme", Some("support")))
        .await
        .unwrap();

    let poster = Arc::new(RecordingPoster::default());
    let scope = tenant_scope("dev", "acme", None);
    let provider = provider_with_secrets(
        Config::with_base_url("https://directline.test/v3/directline"),
        signing_scope(),
        &[(&scope, "webchat", "channel_token", "dl-secret")],
    );
    let state = AppState::new(provider, direct_line, client.clone())
        .with_sessions(sessions)
        .with_activity_poster(poster.clone() as SharedDirectLinePoster)
        .with_oauth_client(Arc::new(StubOauthClient))
        .with_conversations(conversations.clone());

    let Json(payload) = admin_post_activity(
        State(state),
        Path(AdminPath {
            env: "dev".to_string(),
            tenant: "acme".to_string(),
        }),
        Json(AdminPostActivityRequest {
            team: Some("support".to_string()),
            conversation_id: None,
            activity: json!({
                "type": "event",
                "name": "proactive.ping"
            }),
        }),
    )
    .await
    .unwrap();

    assert_eq!(payload.posted, 1);
    assert_eq!(payload.skipped, 1);

    let calls = poster.calls.lock().await;
    assert!(calls.is_empty());

    let page = conversations
        .activities("conv-allow", None)
        .await
        .expect("allow conversation activities");
    assert_eq!(page.activities.len(), 1);
    let stored = &page.activities[0].activity;
    assert_eq!(stored.from.as_ref().unwrap().id, "bot");
    assert_eq!(stored.r#type, "event");
    assert_eq!(stored.text.as_deref(), None);
}

#[tokio::test]
async fn admin_errors_for_unknown_conversation() {
    let direct_line = Arc::new(MockDirectLineApi::default());
    let client = Client::builder().build().unwrap();
    let store = Arc::new(MemorySessionStore::default());
    let sessions: SharedSessionStore = store.clone();
    let conversations = memory_store();

    let poster = Arc::new(RecordingPoster::default());
    let scope = tenant_scope("dev", "acme", None);
    let provider = provider_with_secrets(
        Config::with_base_url("https://directline.test/v3/directline"),
        signing_scope(),
        &[(&scope, "webchat", "channel_token", "dl-secret")],
    );
    let state = AppState::new(provider, direct_line, client.clone())
        .with_sessions(sessions)
        .with_activity_poster(poster.clone() as SharedDirectLinePoster)
        .with_oauth_client(Arc::new(StubOauthClient))
        .with_conversations(conversations.clone());

    let result = admin_post_activity(
        State(state),
        Path(AdminPath {
            env: "dev".to_string(),
            tenant: "acme".to_string(),
        }),
        Json(AdminPostActivityRequest {
            team: None,
            conversation_id: Some("unknown".to_string()),
            activity: json!({
                "type": "message",
                "text": "hello proactive"
            }),
        }),
    )
    .await;

    match result {
        Ok(_) => panic!("expected not found error"),
        Err(WebChatError::NotFound(message)) => {
            assert_eq!(message, "conversation not found");
        }
        Err(other) => panic!("unexpected error: {other:?}"),
    }
}

#[derive(Default)]
struct RecordingPoster {
    calls: Mutex<Vec<(String, Value)>>,
}

#[async_trait::async_trait]
impl DirectLinePoster for RecordingPoster {
    async fn post_activity(
        &self,
        _base_url: &str,
        conversation_id: &str,
        _bearer_token: &str,
        activity: Value,
    ) -> Result<(), DirectLineError> {
        self.calls
            .lock()
            .await
            .push((conversation_id.to_string(), activity));
        Ok(())
    }
}

struct StubOauthClient;

#[async_trait::async_trait]
impl GreenticOauthClient for StubOauthClient {
    async fn exchange_code(
        &self,
        _tenant_ctx: &TenantCtx,
        _config: &OAuthProviderConfig,
        _code: &str,
        _redirect_uri: &str,
    ) -> Result<String, anyhow::Error> {
        Ok("oauth-token-handle".to_string())
    }
}
