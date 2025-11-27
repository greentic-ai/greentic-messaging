use gsm_core::Platform;
use messaging_gateway::InMemoryBusClient;
use messaging_gateway::config::GatewayConfig;
use messaging_gateway::http::{GatewayState, NormalizedRequest, handle_ingress};
use messaging_gateway::load_adapter_registry;
use std::sync::Arc;

fn test_config() -> GatewayConfig {
    GatewayConfig {
        env: "dev".try_into().unwrap(),
        nats_url: "nats://localhost".into(),
        addr: "127.0.0.1:0".parse().unwrap(),
        default_team: "default".into(),
        subject_prefix: gsm_bus::INGRESS_SUBJECT_PREFIX.to_string(),
        worker_routing: None,
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
        worker: None,
        worker_config: None,
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
            payload: serde_json::json!({ "text": "ok" }),
        }],
        timestamp_utc: req.timestamp_utc.clone(),
    });

    let state = Arc::new(GatewayState {
        bus: bus.clone(),
        config,
        adapters,
        worker: Some(Arc::new(worker)),
        worker_config: worker_config.clone(),
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
