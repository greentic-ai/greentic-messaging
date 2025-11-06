use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
};
use greentic_messaging_providers_webchat::{
    config::Config,
    conversation::memory_store,
    directline_client::{DirectLineError, MockDirectLineApi},
    error::WebChatError,
    http::{
        AdminPath, AdminPostActivityRequest, AppState, DirectLinePoster, SharedDirectLinePoster,
        admin_post_activity,
    },
    session::{MemorySessionStore, SharedSessionStore, WebchatSession, WebchatSessionStore},
};
use greentic_types::{EnvId, TeamId, TenantCtx, TenantId};
use reqwest::Client;
use serde_json::{Value, json};
use tokio::sync::Mutex;

fn tenant_ctx(env: &str, tenant: &str, team: Option<&str>) -> TenantCtx {
    let mut ctx = TenantCtx::new(EnvId::from(env), TenantId::from(tenant));
    if let Some(team) = team {
        ctx = ctx.with_team(Some(TeamId::from(team)));
    }
    ctx
}

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
    let state = AppState::new(
        Config::with_base_url("https://directline.test/v3/directline"),
        direct_line,
        client,
    )
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
    let state = AppState::new(
        Config::with_base_url("https://directline.test/v3/directline"),
        direct_line,
        client,
    )
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
    let state = AppState::new(
        Config::with_base_url("https://directline.test/v3/directline"),
        direct_line,
        client,
    )
    .with_sessions(sessions)
    .with_activity_poster(poster.clone() as SharedDirectLinePoster)
    .with_oauth_client(Arc::new(StubOauthClient))
    .with_conversations(conversations);

    let error = match admin_post_activity(
        State(state),
        Path(AdminPath {
            env: "dev".to_string(),
            tenant: "acme".to_string(),
        }),
        Json(AdminPostActivityRequest {
            team: None,
            conversation_id: Some("missing".to_string()),
            activity: json!({
                "type": "message",
                "text": "hello"
            }),
        }),
    )
    .await
    {
        Ok(_) => panic!("expected error"),
        Err(err) => err,
    };

    match error {
        WebChatError::NotFound(message) => assert_eq!(message, "conversation not found"),
        other => panic!("unexpected error: {other:?}"),
    }
    assert!(poster.calls.lock().await.is_empty());
}

#[derive(Default)]
struct RecordingPoster {
    calls: Mutex<Vec<RecordedActivity>>,
}

#[allow(dead_code)]
struct RecordedActivity {
    conversation_id: String,
    bearer_token: String,
    activity: Value,
}

#[async_trait::async_trait]
impl DirectLinePoster for RecordingPoster {
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

struct StubOauthClient;

#[async_trait::async_trait]
impl greentic_messaging_providers_webchat::oauth::GreenticOauthClient for StubOauthClient {
    async fn exchange_code(
        &self,
        _: &TenantCtx,
        _: &greentic_messaging_providers_webchat::config::OAuthProviderConfig,
        _: &str,
        _: &str,
    ) -> Result<String, anyhow::Error> {
        Ok("stub-handle".to_string())
    }
}
