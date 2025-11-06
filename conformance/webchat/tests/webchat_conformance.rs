use std::collections::VecDeque;
use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    response::IntoResponse,
};
use greentic_messaging_providers_webchat::{
    bus::{EventBus, Subject},
    circuit::CircuitSettings,
    config::Config,
    conversation::memory_store,
    directline_client::{DirectLineError, MockDirectLineApi},
    http::{
        AdminPath, AdminPostActivityRequest, AppState, DirectLinePoster, SharedDirectLinePoster,
        admin_post_activity,
    },
    ingress::{
        ActivitiesEnvelope, ActivitiesTransport, ActivitiesTransportResponse, IngressCtx,
        IngressDeps, SharedActivitiesTransport, run_poll_loop,
    },
    oauth::{CallbackQuery, GreenticOauthClient},
    session::{MemorySessionStore, SharedSessionStore, WebchatSession},
    types::{GreenticEvent, IncomingMessage, MessagePayload},
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

fn build_state(
    config: Config,
    direct_line: Arc<MockDirectLineApi>,
    sessions: SharedSessionStore,
    transport: SharedActivitiesTransport,
    poster: SharedDirectLinePoster,
    oauth: Arc<dyn GreenticOauthClient>,
) -> AppState {
    let client = Client::builder().build().unwrap();
    AppState::new(config, direct_line, client.clone())
        .with_sessions(sessions)
        .with_transport(transport)
        .with_activity_poster(poster)
        .with_oauth_client(oauth)
}

#[tokio::test]
async fn text_activity_publishes_incoming_event() {
    let bus = Arc::new(RecordingBus::default());
    let sessions: SharedSessionStore = Arc::new(MemorySessionStore::default());
    let transport = Arc::new(ScriptedTransport::new(vec![
        ActivitiesTransportResponse {
            status: http::StatusCode::OK,
            body: Some(ActivitiesEnvelope {
                activities: vec![json!({
                    "type": "message",
                    "id": "activity-1",
                    "timestamp": "2024-05-01T12:00:00Z",
                    "text": "Hello from Web Chat!",
                    "from": { "id": "user-42", "name": "Mina" }
                })],
                watermark: Some("13".to_string()),
            }),
        },
        ActivitiesTransportResponse {
            status: http::StatusCode::UNAUTHORIZED,
            body: None,
        },
    ]));

    let deps = IngressDeps {
        bus: bus.clone(),
        sessions: sessions.clone(),
        dl_base: "https://directline.test/v3/directline".into(),
        transport,
        circuit: CircuitSettings::default(),
    };
    let ctx = IngressCtx {
        tenant_ctx: tenant_ctx("Dev", "Acme", Some("Support")),
        conversation_id: "conv-text".into(),
        token: "dl-token-text".into(),
    };

    run_poll_loop(deps, ctx).await.unwrap();

    let events = bus.events.lock().await;
    assert_eq!(events.len(), 1);

    let (subject, event) = &events[0];
    assert_eq!(subject, "greentic.dev.acme.support.events.incoming");

    match event {
        GreenticEvent::IncomingMessage(IncomingMessage {
            payload: MessagePayload::Text { text, .. },
            ..
        }) => assert_eq!(text, "Hello from Web Chat!"),
        other => panic!("unexpected payload: {other:?}"),
    }
}

#[tokio::test]
async fn adaptive_card_and_invoke_are_normalised() {
    let bus = Arc::new(RecordingBus::default());
    let sessions: SharedSessionStore = Arc::new(MemorySessionStore::default());
    let transport = Arc::new(ScriptedTransport::new(vec![
        ActivitiesTransportResponse {
            status: http::StatusCode::OK,
            body: Some(ActivitiesEnvelope {
                activities: vec![
                    json!({
                        "type": "message",
                        "id": "ac-message",
                        "timestamp": "2024-05-01T12:30:00Z",
                        "attachments": [{
                            "contentType": "application/vnd.microsoft.card.adaptive",
                            "content": {
                                "type": "AdaptiveCard",
                                "version": "1.5",
                                "body": [
                                    { "type": "TextBlock", "text": "Do you approve?" },
                                    { "type": "Input.Text", "id": "comment" }
                                ]
                            }
                        }],
                        "from": { "id": "bot" }
                    }),
                    json!({
                        "type": "invoke",
                        "name": "adaptiveCard/action",
                        "id": "ac-invoke",
                        "timestamp": "2024-05-01T12:30:05Z",
                        "value": {
                            "action": {
                                "type": "Action.Submit",
                                "data": { "comment": "Looks good" }
                            }
                        },
                        "from": { "id": "user", "name": "Alex" }
                    }),
                ],
                watermark: Some("77".to_string()),
            }),
        },
        ActivitiesTransportResponse {
            status: http::StatusCode::UNAUTHORIZED,
            body: None,
        },
    ]));

    run_poll_loop(
        IngressDeps {
            bus: bus.clone(),
            sessions,
            dl_base: "https://directline.test/v3/directline".into(),
            transport,
            circuit: CircuitSettings::default(),
        },
        IngressCtx {
            tenant_ctx: tenant_ctx("DEV", "ACME", None),
            conversation_id: "conv-ac".into(),
            token: "dl-token-ac".into(),
        },
    )
    .await
    .unwrap();

    let events = bus.events.lock().await;
    assert_eq!(events.len(), 2);

    match &events[0].1 {
        GreenticEvent::IncomingMessage(IncomingMessage {
            payload: MessagePayload::Attachment { content_type, .. },
            ..
        }) => assert_eq!(content_type, "application/vnd.microsoft.card.adaptive"),
        other => panic!("unexpected first payload: {other:?}"),
    }

    match &events[1].1 {
        GreenticEvent::IncomingMessage(IncomingMessage {
            payload: MessagePayload::Event { name, value },
            ..
        }) => {
            assert_eq!(name, "adaptiveCard/action");
            assert_eq!(
                value
                    .as_ref()
                    .and_then(|v| v.pointer("/action/data/comment"))
                    .and_then(Value::as_str),
                Some("Looks good")
            );
        }
        other => panic!("unexpected second payload: {other:?}"),
    }
}

#[tokio::test]
async fn oauth_callback_posts_token_handle() {
    let poster = Arc::new(RecordingPoster::default());
    let sessions: SharedSessionStore = Arc::new(MemorySessionStore::default());
    sessions
        .upsert(WebchatSession::new(
            "conv-oauth".to_string(),
            tenant_ctx("dev", "acme", None),
            "directline-token".to_string(),
        ))
        .await
        .unwrap();

    let oauth_map: Arc<std::collections::HashMap<String, String>> = Arc::new(
        [
            (
                "WEBCHAT_OAUTH_ISSUER__DEV__ACME".to_string(),
                "https://oauth.greentic.dev".to_string(),
            ),
            (
                "WEBCHAT_OAUTH_CLIENT_ID__DEV__ACME".to_string(),
                "client-xyz".to_string(),
            ),
            (
                "WEBCHAT_OAUTH_REDIRECT_BASE__DEV__ACME".to_string(),
                "https://messaging.greentic.dev".to_string(),
            ),
        ]
        .into_iter()
        .collect(),
    );

    let conversations = memory_store();
    let state = build_state(
        Config::with_base_url("https://directline.test/v3/directline").with_oauth_lookup({
            let map = Arc::clone(&oauth_map);
            move |key| map.get(key).cloned()
        }),
        Arc::new(MockDirectLineApi::default()),
        sessions,
        Arc::new(ScriptedTransport::default()),
        poster.clone() as SharedDirectLinePoster,
        Arc::new(StubOauthClient),
    )
    .with_conversations(conversations.clone());

    let response = greentic_messaging_providers_webchat::oauth::callback(
        State(state),
        axum::extract::Query(CallbackQuery {
            conversation_id: "conv-oauth".to_string(),
            code: Some("auth-code-xyz".to_string()),
            state: Some("bf-state".to_string()),
            error: None,
        }),
    )
    .await
    .unwrap();

    let body = response.into_response().into_body();
    let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    assert_eq!(
        bytes,
        greentic_messaging_providers_webchat::oauth::CLOSE_WINDOW_HTML.as_bytes()
    );

    let page = conversations
        .activities("conv-oauth", None)
        .await
        .expect("conversation activities");
    assert_eq!(page.activities.len(), 1);
    let stored = &page.activities[0].activity;
    assert_eq!(stored.from.as_ref().map(|f| f.id.as_str()), Some("bot"));
    assert_eq!(
        stored
            .channel_data
            .as_ref()
            .and_then(|value| value.get("oauth_token_handle"))
            .and_then(Value::as_str),
        Some("stub-handle")
    );
}

#[tokio::test]
async fn admin_endpoint_posts_and_skips_sessions() {
    let poster = Arc::new(RecordingPoster::default());
    let sessions: SharedSessionStore = Arc::new(MemorySessionStore::default());
    let conversations = memory_store();
    sessions
        .upsert(WebchatSession::new(
            "conv-a".to_string(),
            tenant_ctx("dev", "acme", Some("support")),
            "token-a".to_string(),
        ))
        .await
        .unwrap();
    conversations
        .create("conv-a", tenant_ctx("dev", "acme", Some("support")))
        .await
        .unwrap();
    sessions
        .upsert(WebchatSession::new(
            "conv-b".to_string(),
            tenant_ctx("dev", "acme", Some("support")),
            "token-b".to_string(),
        ))
        .await
        .unwrap();
    sessions.set_proactive("conv-b", false).await.unwrap();

    let state = build_state(
        Config::with_base_url("https://directline.test/v3/directline"),
        Arc::new(MockDirectLineApi::default()),
        sessions,
        Arc::new(ScriptedTransport::default()),
        poster.clone() as SharedDirectLinePoster,
        Arc::new(StubOauthClient),
    )
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

    let page = conversations
        .activities("conv-a", None)
        .await
        .expect("stored proactive activity");
    assert_eq!(page.activities.len(), 1);
    let stored = &page.activities[0].activity;
    assert_eq!(stored.r#type, "event");
    assert_eq!(
        stored.from.as_ref().map(|from| from.id.as_str()),
        Some("bot")
    );
}

#[derive(Default)]
struct RecordingPoster {
    calls: Mutex<Vec<RecordedActivity>>,
}

#[allow(dead_code)]
struct RecordedActivity {
    conversation_id: String,
    _bearer_token: String,
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
            _bearer_token: bearer_token.to_string(),
            activity,
        });
        Ok(())
    }
}

#[derive(Default)]
struct RecordingBus {
    events: Mutex<Vec<(String, GreenticEvent)>>,
}

#[async_trait::async_trait]
impl EventBus for RecordingBus {
    async fn publish(&self, subject: &Subject, event: &GreenticEvent) -> anyhow::Result<()> {
        self.events
            .lock()
            .await
            .push((subject.as_str().to_string(), event.clone()));
        Ok(())
    }
}

#[derive(Default)]
struct ScriptedTransport {
    responses: Mutex<VecDeque<ActivitiesTransportResponse>>,
}

impl ScriptedTransport {
    fn new(responses: Vec<ActivitiesTransportResponse>) -> Self {
        Self {
            responses: Mutex::new(responses.into()),
        }
    }
}

#[async_trait::async_trait]
impl ActivitiesTransport for ScriptedTransport {
    async fn poll(
        &self,
        _dl_base: &str,
        _conversation_id: &str,
        _token: &str,
        _watermark: Option<&str>,
    ) -> anyhow::Result<ActivitiesTransportResponse> {
        let mut guard = self.responses.lock().await;
        Ok(guard.pop_front().unwrap_or(ActivitiesTransportResponse {
            status: http::StatusCode::OK,
            body: None,
        }))
    }
}

struct StubOauthClient;

#[async_trait::async_trait]
impl GreenticOauthClient for StubOauthClient {
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
