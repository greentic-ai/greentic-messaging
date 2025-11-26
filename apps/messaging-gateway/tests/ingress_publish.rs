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
        subject_prefix: messaging_bus::INGRESS_SUBJECT_PREFIX.to_string(),
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
