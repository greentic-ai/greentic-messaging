use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use greentic_messaging_providers_webchat::activity_bridge::normalize_activity;
use greentic_messaging_providers_webchat::bus::{EventBus, Subject};
use greentic_messaging_providers_webchat::circuit::CircuitSettings;
use greentic_messaging_providers_webchat::ingress::{
    ActivitiesEnvelope, ActivitiesTransport, ActivitiesTransportResponse, IngressCtx, IngressDeps,
    SharedActivitiesTransport, run_poll_loop,
};
use greentic_messaging_providers_webchat::session::{
    MemorySessionStore, SharedSessionStore, WebchatSession, WebchatSessionStore,
};
use greentic_messaging_providers_webchat::types::{GreenticEvent, IncomingMessage, MessagePayload};
use greentic_types::{EnvId, TenantCtx, TenantId};
use reqwest::StatusCode;
use serde_json::{Value, json};
use tokio::sync::Mutex;
use tokio::time::Instant;

struct RecordingBus {
    events: Mutex<Vec<(String, GreenticEvent)>>,
}

impl RecordingBus {
    fn new() -> Self {
        Self {
            events: Mutex::new(Vec::new()),
        }
    }
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

fn tenant_ctx() -> TenantCtx {
    TenantCtx::new(EnvId::from("dev"), TenantId::from("acme"))
}

fn webchat_session() -> WebchatSession {
    WebchatSession::new("conv-1".to_string(), tenant_ctx(), "token".into())
}

#[tokio::test]
async fn normalize_activity_variants() {
    let session = webchat_session();
    let cases: Vec<(&str, Value, Box<dyn Fn(&IncomingMessage) + Send + Sync>)> = vec![
        (
            "text",
            json!({
                "type": "message",
                "id": "msg-1",
                "timestamp": "2024-01-01T00:00:00Z",
                "text": "hello",
                "locale": "en-US",
                "from": {"id": "user-1"}
            }),
            Box::new(|msg| match &msg.payload {
                MessagePayload::Text { text, locale } => {
                    assert_eq!(text, "hello");
                    assert_eq!(locale.as_deref(), Some("en-US"));
                }
                other => panic!("unexpected payload: {other:?}"),
            }),
        ),
        (
            "typing",
            json!({
                "type": "typing",
                "id": "typing-1",
                "timestamp": "2024-01-01T00:00:01Z",
                "from": {"id": "user-1", "name": "Greentic"}
            }),
            Box::new(|msg| {
                assert!(matches!(msg.payload, MessagePayload::Typing));
            }),
        ),
        (
            "event",
            json!({
                "type": "event",
                "name": "handoff",
                "value": {"dest": "agent"},
                "id": "evt-1",
                "timestamp": "2024-01-01T00:00:02Z",
                "from": {"id": "system"}
            }),
            Box::new(|msg| match &msg.payload {
                MessagePayload::Event { name, value } => {
                    assert_eq!(name, "handoff");
                    assert!(value.is_some());
                }
                other => panic!("unexpected payload: {other:?}"),
            }),
        ),
        (
            "attachment",
            json!({
                "type": "message",
                "id": "msg-2",
                "timestamp": "2024-01-01T00:00:03Z",
                "attachments": [
                    {"contentType": "application/vnd.microsoft.card.adaptive", "content": {"title": "Card"}}
                ],
                "from": {"id": "bot"}
            }),
            Box::new(|msg| match &msg.payload {
                MessagePayload::Attachment {
                    content_type,
                    content,
                } => {
                    assert_eq!(content_type, "application/vnd.microsoft.card.adaptive");
                    assert_eq!(content["title"], "Card");
                }
                other => panic!("unexpected payload: {other:?}"),
            }),
        ),
    ];

    for (name, activity, assert_fn) in cases {
        let result = normalize_activity(&session, &activity).expect(name);
        assert_eq!(result.from.id, activity["from"]["id"].as_str().unwrap());
        assert_fn(&result);
    }
}

#[tokio::test]
async fn poll_loop_updates_watermark_and_publishes() {
    let responses = vec![
        ok_response(
            vec![json!({
                "type": "message",
                "id": "m-1",
                "timestamp": "2024-01-01T00:00:00Z",
                "text": "hello",
                "from": {"id": "user"}
            })],
            Some("10"),
        ),
        status_response(StatusCode::UNAUTHORIZED),
    ];
    let (transport, counter) = queue_transport(responses);

    let bus = Arc::new(RecordingBus::new());
    let store = Arc::new(MemorySessionStore::default());
    let sessions: SharedSessionStore = store.clone();
    let deps = IngressDeps {
        bus: bus.clone(),
        sessions: sessions.clone(),
        dl_base: "https://directline.example/v3/directline".into(),
        transport,
        circuit: CircuitSettings::default(),
    };
    let ctx = IngressCtx {
        tenant_ctx: tenant_ctx(),
        conversation_id: "conv-1".into(),
        token: "token".into(),
    };

    run_poll_loop(deps, ctx).await.unwrap();

    let stored = store.get("conv-1").await.unwrap().unwrap();
    assert_eq!(stored.watermark.as_deref(), Some("10"));
    let events = bus.events.lock().await;
    assert_eq!(events.len(), 1);
    assert_eq!(counter.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn poll_loop_retries_with_backoff() {
    let responses = vec![
        status_response(StatusCode::TOO_MANY_REQUESTS),
        status_response(StatusCode::INTERNAL_SERVER_ERROR),
        ok_response(
            vec![json!({
                "type": "typing",
                "id": "m-2",
                "timestamp": "2024-01-01T00:00:00Z",
                "from": {"id": "user"}
            })],
            Some("11"),
        ),
        status_response(StatusCode::FORBIDDEN),
    ];
    let (transport, counter) = queue_transport(responses);

    let sessions: SharedSessionStore = Arc::new(MemorySessionStore::default());
    let deps = IngressDeps {
        bus: Arc::new(RecordingBus::new()),
        sessions,
        dl_base: "https://directline.example/v3/directline".into(),
        transport,
        circuit: CircuitSettings::default(),
    };
    let ctx = IngressCtx {
        tenant_ctx: tenant_ctx(),
        conversation_id: "conv-2".into(),
        token: "token".into(),
    };

    let start = Instant::now();
    run_poll_loop(deps, ctx).await.unwrap();
    assert!(counter.load(Ordering::SeqCst) >= 3);
    assert!(start.elapsed() >= Duration::from_millis(10));
}

fn ok_response(activities: Vec<Value>, watermark: Option<&str>) -> ActivitiesTransportResponse {
    ActivitiesTransportResponse {
        status: StatusCode::OK,
        body: Some(ActivitiesEnvelope {
            activities,
            watermark: watermark.map(|w| w.to_string()),
        }),
    }
}

fn status_response(status: StatusCode) -> ActivitiesTransportResponse {
    ActivitiesTransportResponse { status, body: None }
}

fn queue_transport(
    responses: Vec<ActivitiesTransportResponse>,
) -> (SharedActivitiesTransport, Arc<AtomicUsize>) {
    let counter = Arc::new(AtomicUsize::new(0));
    let transport: SharedActivitiesTransport = Arc::new(MockTransport {
        responses: Mutex::new(VecDeque::from(responses)),
        counter: counter.clone(),
    });
    (transport, counter)
}

struct MockTransport {
    responses: Mutex<VecDeque<ActivitiesTransportResponse>>,
    counter: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl ActivitiesTransport for MockTransport {
    async fn poll(
        &self,
        _: &str,
        _: &str,
        _: &str,
        _: Option<&str>,
    ) -> anyhow::Result<ActivitiesTransportResponse> {
        self.counter.fetch_add(1, Ordering::SeqCst);
        let mut guard = self.responses.lock().await;
        Ok(guard
            .pop_front()
            .unwrap_or_else(|| status_response(StatusCode::INTERNAL_SERVER_ERROR)))
    }
}
