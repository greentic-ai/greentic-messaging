use std::str::FromStr;
use std::sync::Arc;

use gsm_bus::InMemoryBusClient;
use gsm_core::{
    AdapterDescriptor, ChannelMessage, MessagingAdapterKind, OutKind, OutMessage, Platform,
    make_tenant_ctx,
};
use gsm_gateway::config::GatewayConfig;
use gsm_gateway::http::{GatewayState, NormalizedRequest, handle_ingress};
use gsm_gateway::load_adapter_registry;
use messaging_egress::adapter_registry::AdapterLookup;
use messaging_egress::process_message_internal;

fn test_gateway_config() -> GatewayConfig {
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

fn adapter(name: &str, kind: MessagingAdapterKind) -> AdapterDescriptor {
    AdapterDescriptor {
        pack_id: "pack".into(),
        pack_version: "1.0.0".into(),
        name: name.into(),
        kind,
        component: "comp@1.0.0".into(),
        default_flow: None,
        custom_flow: None,
        capabilities: None,
        source: None,
    }
}

#[tokio::test]
async fn ingress_to_egress_round_trip_over_in_memory_bus() {
    let bus = Arc::new(InMemoryBusClient::default());
    let adapters = load_adapter_registry();
    let state = Arc::new(GatewayState {
        bus: bus.clone(),
        config: test_gateway_config(),
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

    let _resp = handle_ingress(
        "acme".into(),
        Some("team".into()),
        "slack".into(),
        state,
        payload,
        Default::default(),
    )
    .await
    .expect("ingress should succeed");

    let published_ingress = bus.take_published().await;
    assert_eq!(published_ingress.len(), 1);
    let (subject, raw) = &published_ingress[0];
    assert!(subject.contains("greentic.messaging.ingress.dev.acme.team.slack"));
    let channel: ChannelMessage = serde_json::from_value(raw.clone()).unwrap();
    let text = channel.payload["text"].as_str().unwrap();

    let mut registry = gsm_core::AdapterRegistry::default();
    registry
        .register(adapter("slack-main", MessagingAdapterKind::IngressEgress))
        .unwrap();
    let lookup = AdapterLookup::new(&registry);
    let adapter = lookup
        .default_for_platform(Platform::Slack.as_str())
        .unwrap();

    let out = OutMessage {
        ctx: make_tenant_ctx("dev".into(), Some("acme".into()), None),
        tenant: "acme".into(),
        platform: Platform::from_str(channel.channel_id.as_str()).unwrap(),
        chat_id: channel.session_id.clone(),
        thread_id: None,
        kind: OutKind::Text,
        text: Some(text.to_string()),
        message_card: None,
        adaptive_card: None,
        meta: Default::default(),
    };

    let cfg = messaging_egress::config::EgressConfig {
        env: "dev".try_into().unwrap(),
        nats_url: "nats://localhost".into(),
        subject_filter: "greentic.messaging.egress.dev.>".into(),
        adapter: None,
        packs_root: "packs".into(),
        egress_prefix: gsm_bus::EGRESS_SUBJECT_PREFIX.to_string(),
    };

    process_message_internal(&out, &adapter, bus.as_ref(), &cfg)
        .await
        .unwrap();

    let published = bus.take_published().await;
    assert_eq!(published.len(), 1);
    let (egress_subject, payload) = &published[0];
    assert!(egress_subject.contains("greentic.messaging.egress.out.acme.slack"));
    assert_eq!(payload["text"], text);
    assert_eq!(payload["adapter"], "slack-main");
}
