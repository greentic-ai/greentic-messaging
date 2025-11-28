use gsm_core::Platform;
use gsm_gateway::InMemoryBusClient;
use gsm_gateway::config::GatewayConfig;
use gsm_gateway::http::{GatewayState, NormalizedRequest, handle_ingress};
use gsm_gateway::load_adapter_registry;
use std::sync::Arc;

fn test_config() -> GatewayConfig {
    GatewayConfig {
        env: "dev".try_into().unwrap(),
        nats_url: "nats://localhost".into(),
        addr: "127.0.0.1:0".parse().unwrap(),
        default_team: "default".into(),
        subject_prefix: gsm_bus::INGRESS_SUBJECT_PREFIX.to_string(),
        worker_routing: None,
        worker_routes: std::collections::BTreeMap::new(),
        worker_egress_subject: None,
    }
}

#[tokio::test]
async fn ingress_is_normalized_and_published() {
    let bus = Arc::new(InMemoryBusClient::default());
    let adapters = load_adapter_registry();
    let state = Arc::new(GatewayState {
        bus: bus.clone(),
        config: test_config(),
        adapters,
        workers: std::collections::BTreeMap::new(),
        worker_default: None,
        worker_egress_subject: None,
    });

    let payload = NormalizedRequest {
        chat_id: Some("chat-1".into()),
        user_id: Some("user-1".into()),
        text: Some("hi".into()),
        ..Default::default()
    };

    let _ = handle_ingress(
        "acme".into(),
        Some("team".into()),
        "slack".into(),
        state,
        payload,
        Default::default(),
    )
    .await
    .expect("ingress should succeed");

    let published = bus.take_published().await;
    assert_eq!(published.len(), 1);
    let (subject, raw) = &published[0];
    assert!(subject.contains("greentic.messaging.ingress.dev.acme.team.slack"));
    let msg: gsm_core::ChannelMessage = serde_json::from_value(raw.clone()).unwrap();
    assert_eq!(msg.channel_id, Platform::Slack.as_str());
    assert_eq!(msg.session_id, "chat-1");
    assert_eq!(msg.route, None);
    assert_eq!(msg.tenant.tenant.as_str(), "acme");
    assert_eq!(msg.payload["text"], "hi");
    assert_eq!(msg.payload["user_id"], "user-1");
}

#[tokio::test]
async fn forwards_multiple_worker_messages_including_card() {
    let bus = Arc::new(InMemoryBusClient::default());
    let adapters = load_adapter_registry();
    let mut config = test_config();
    config.worker_egress_subject = Some("greentic.messaging.egress.dev.repo-worker".into());
    let worker_cfg = gsm_core::WorkerRoutingConfig::default();
    let worker = gsm_core::InMemoryWorkerClient::new(|req| {
        let mut resp = gsm_core::empty_worker_response_for(&req);
        resp.messages = vec![
            gsm_core::WorkerMessage {
                kind: "text".into(),
                payload_json: serde_json::to_string(&serde_json::json!({ "text": "one" })).unwrap(),
            },
            gsm_core::WorkerMessage {
                kind: "card".into(),
                payload_json: serde_json::to_string(&serde_json::json!({
                    "card": { "title": "two" }
                }))
                .unwrap(),
            },
        ];
        resp
    });
    let mut workers = std::collections::BTreeMap::new();
    workers.insert(
        worker_cfg.worker_id.clone(),
        Arc::new(worker) as Arc<dyn gsm_core::WorkerClient>,
    );

    let state = Arc::new(GatewayState {
        bus: bus.clone(),
        config,
        adapters,
        workers,
        worker_default: Some(worker_cfg),
        worker_egress_subject: Some("greentic.messaging.egress.dev.repo-worker".into()),
    });

    let payload = NormalizedRequest {
        chat_id: Some("chat-1".into()),
        user_id: Some("user-1".into()),
        text: Some("hi".into()),
        ..Default::default()
    };

    let _ = handle_ingress(
        "acme".into(),
        Some("team".into()),
        "slack".into(),
        state,
        payload,
        Default::default(),
    )
    .await
    .expect("ingress should succeed");

    let published = bus.take_published().await;
    assert_eq!(published.len(), 3);
    let (_subject, raw_ingress) = &published[0];
    let ingress: gsm_core::ChannelMessage = serde_json::from_value(raw_ingress.clone()).unwrap();
    assert_eq!(ingress.payload["text"], "hi");

    let (_subject2, raw_out1) = &published[1];
    let out1: gsm_core::OutMessage = serde_json::from_value(raw_out1.clone()).unwrap();
    assert_eq!(out1.kind, gsm_core::OutKind::Text);
    assert_eq!(out1.text.as_deref(), Some("one"));

    let (_subject3, raw_out2) = &published[2];
    let out2: gsm_core::OutMessage = serde_json::from_value(raw_out2.clone()).unwrap();
    assert_eq!(out2.kind, gsm_core::OutKind::Card);
    assert!(out2.text.is_none());
    assert_eq!(
        out2.meta.get("worker_payload").unwrap()["card"]["title"],
        "two"
    );
}

struct FailingWorker;

#[async_trait::async_trait]
impl gsm_core::WorkerClient for FailingWorker {
    async fn send_request(
        &self,
        _request: gsm_core::WorkerRequest,
    ) -> Result<gsm_core::WorkerResponse, gsm_core::WorkerClientError> {
        Err(gsm_core::WorkerClientError::Http(anyhow::anyhow!("boom")))
    }
}

#[tokio::test]
async fn worker_failure_does_not_block_ingress_publish() {
    let bus = Arc::new(InMemoryBusClient::default());
    let adapters = load_adapter_registry();
    let mut config = test_config();
    config.worker_egress_subject = Some("greentic.messaging.egress.dev.repo-worker".into());
    let worker_cfg = gsm_core::WorkerRoutingConfig::default();
    let mut workers = std::collections::BTreeMap::new();
    workers.insert(
        worker_cfg.worker_id.clone(),
        Arc::new(FailingWorker) as Arc<dyn gsm_core::WorkerClient>,
    );

    let state = Arc::new(GatewayState {
        bus: bus.clone(),
        config,
        adapters,
        workers,
        worker_default: Some(worker_cfg),
        worker_egress_subject: Some("greentic.messaging.egress.dev.repo-worker".into()),
    });

    let payload = NormalizedRequest {
        chat_id: Some("chat-1".into()),
        user_id: Some("user-1".into()),
        text: Some("hi".into()),
        ..Default::default()
    };

    let _ = handle_ingress(
        "acme".into(),
        Some("team".into()),
        "slack".into(),
        state,
        payload,
        Default::default(),
    )
    .await
    .expect("ingress should succeed even if worker fails");

    let published = bus.take_published().await;
    // Only the ingress publish succeeds; worker publishes are skipped on failure.
    assert_eq!(published.len(), 1);
}
#[tokio::test]
async fn forwards_to_worker_when_configured() {
    let bus = Arc::new(InMemoryBusClient::default());
    let adapters = load_adapter_registry();
    let mut config = test_config();
    config.worker_egress_subject = Some("greentic.messaging.egress.dev.repo-worker".into());
    let worker_config = Some(gsm_core::WorkerRoutingConfig::default());
    let worker = gsm_core::InMemoryWorkerClient::new(|req| gsm_core::WorkerResponse {
        version: req.version.clone(),
        tenant: req.tenant.clone(),
        worker_id: req.worker_id.clone(),
        correlation_id: req.correlation_id.clone(),
        session_id: req.session_id.clone(),
        thread_id: req.thread_id.clone(),
        messages: vec![gsm_core::WorkerMessage {
            kind: "text".into(),
            payload_json: serde_json::to_string(&serde_json::json!({ "text": "ok" })).unwrap(),
        }],
        timestamp_utc: req.timestamp_utc.clone(),
    });

    let state = Arc::new(GatewayState {
        bus: bus.clone(),
        config,
        adapters,
        workers: {
            let mut map = std::collections::BTreeMap::new();
            map.insert(
                worker_config.as_ref().unwrap().worker_id.clone(),
                Arc::new(worker) as Arc<dyn gsm_core::WorkerClient>,
            );
            map
        },
        worker_default: worker_config.clone(),
        worker_egress_subject: Some("greentic.messaging.egress.dev.repo-worker".into()),
    });

    let payload = NormalizedRequest {
        chat_id: Some("chat-1".into()),
        user_id: Some("user-1".into()),
        text: Some("hi".into()),
        ..Default::default()
    };

    let _ = handle_ingress(
        "acme".into(),
        Some("team".into()),
        "slack".into(),
        state,
        payload,
        Default::default(),
    )
    .await
    .expect("ingress should succeed");

    let published = bus.take_published().await;
    assert_eq!(published.len(), 2);
    let (_, worker_raw) = &published[1];
    let out: gsm_core::OutMessage = serde_json::from_value(worker_raw.clone()).unwrap();
    assert_eq!(out.text.as_deref(), Some("ok"));
    assert_eq!(out.platform.as_str(), "slack");
}
