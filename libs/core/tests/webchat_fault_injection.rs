use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use greentic_types::{EnvId, TeamId, TenantCtx, TenantId};
use gsm_core::platforms::webchat::{
    bus::{EventBus, Subject},
    circuit::CircuitSettings,
    ingress::{
        ActivitiesEnvelope, ActivitiesTransport, ActivitiesTransportResponse, IngressCtx,
        IngressDeps, SharedActivitiesTransport, run_poll_loop,
    },
    session::{MemorySessionStore, SharedSessionStore, WebchatSession},
    types::GreenticEvent,
};
use http::StatusCode;
use serde_json::json;
use tokio::sync::Mutex;

fn tenant_ctx() -> TenantCtx {
    TenantCtx::new(EnvId("dev".to_string()), TenantId("acme".to_string()))
        .with_team(Some(TeamId("support".to_string())))
}

#[tokio::test]
#[tracing_test::traced_test]
async fn circuit_opens_and_half_opens_without_leaking_tokens() {
    tokio::time::pause();

    let transport = Arc::new(FaultyTransport::new(vec![
        ActivitiesTransportResponse {
            status: StatusCode::TOO_MANY_REQUESTS,
            body: None,
        },
        ActivitiesTransportResponse {
            status: StatusCode::BAD_GATEWAY,
            body: None,
        },
        ActivitiesTransportResponse {
            status: StatusCode::OK,
            body: Some(ActivitiesEnvelope {
                activities: vec![json!({
                    "type": "message",
                    "id": "ok-activity",
                    "timestamp": "2024-04-02T12:00:00Z",
                    "text": "hello",
                    "from": {"id": "bot"}
                })],
                watermark: Some("42".to_string()),
            }),
        },
        ActivitiesTransportResponse {
            status: StatusCode::UNAUTHORIZED,
            body: None,
        },
    ]));

    let sessions: SharedSessionStore = Arc::new(MemorySessionStore::default());
    sessions
        .upsert(WebchatSession::new(
            "conv-fault".to_string(),
            tenant_ctx(),
            "SECRET_TOKEN_123".to_string(),
        ))
        .await
        .unwrap();

    let deps = IngressDeps {
        bus: Arc::new(RecordingBus::default()),
        sessions,
        dl_base: "https://directline.test/v3/directline".into(),
        transport: transport.clone() as SharedActivitiesTransport,
        circuit: CircuitSettings {
            failure_threshold: 2,
            open_duration: std::time::Duration::from_secs(1),
        },
    };

    let ctx = IngressCtx {
        tenant_ctx: tenant_ctx(),
        conversation_id: "conv-fault".into(),
        token: "SECRET_TOKEN_123".into(),
    };

    let handle = tokio::spawn(run_poll_loop(deps, ctx));

    tokio::task::yield_now().await;
    tokio::time::advance(std::time::Duration::from_secs(2)).await;
    tokio::task::yield_now().await;
    tokio::time::advance(std::time::Duration::from_secs(2)).await;
    tokio::task::yield_now().await;
    assert_eq!(transport.poll_count(), 2);

    tokio::time::advance(std::time::Duration::from_millis(500)).await;
    tokio::task::yield_now().await;
    assert_eq!(transport.poll_count(), 2);

    tokio::time::advance(std::time::Duration::from_secs(1)).await;
    tokio::task::yield_now().await;

    handle.await.unwrap().unwrap();

    logs_assert(|lines: &[&str]| {
        let opened = lines
            .iter()
            .any(|line| line.contains("circuit breaker opened"));
        let closed = lines
            .iter()
            .any(|line| line.contains("circuit breaker closed"));
        let leaked = lines.iter().any(|line| line.contains("SECRET_TOKEN_123"));
        if !opened {
            return Err(format!("expected open log, lines: {:?}", lines));
        }
        if !closed {
            return Err(format!("expected close log, lines: {:?}", lines));
        }
        if leaked {
            return Err("secret token leaked into logs".into());
        }
        Ok(())
    });
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

struct FaultyTransport {
    responses: Mutex<VecDeque<ActivitiesTransportResponse>>,
    polls: Arc<AtomicUsize>,
}

impl FaultyTransport {
    fn new(responses: Vec<ActivitiesTransportResponse>) -> Self {
        Self {
            responses: Mutex::new(responses.into()),
            polls: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn poll_count(&self) -> usize {
        self.polls.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl ActivitiesTransport for FaultyTransport {
    async fn poll(
        &self,
        _dl_base: &str,
        _conversation_id: &str,
        _token: &str,
        _watermark: Option<&str>,
    ) -> anyhow::Result<ActivitiesTransportResponse> {
        self.polls.fetch_add(1, Ordering::SeqCst);
        let mut guard = self.responses.lock().await;
        Ok(guard.pop_front().unwrap_or(ActivitiesTransportResponse {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            body: None,
        }))
    }
}
