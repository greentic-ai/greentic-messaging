use gsm_core::{AdapterDescriptor, MessagingAdapterKind};
use gsm_core::{OutKind, OutMessage, Platform, make_tenant_ctx};
use messaging_egress::InMemoryBusClient;
use messaging_egress::adapter_registry::AdapterLookup;

fn adapter(name: &str) -> AdapterDescriptor {
    AdapterDescriptor {
        pack_id: "pack".into(),
        pack_version: "1.0.0".into(),
        name: name.into(),
        kind: MessagingAdapterKind::IngressEgress,
        component: "comp@1.0.0".into(),
        default_flow: None,
        custom_flow: None,
        capabilities: None,
        source: None,
    }
}

#[tokio::test]
async fn publishes_outbound_payload_via_bus() {
    let mut registry = gsm_core::AdapterRegistry::default();
    registry.register(adapter("slack-main")).unwrap();
    let lookup = AdapterLookup::new(&registry);
    let bus = InMemoryBusClient::default();

    let adapter = lookup
        .default_for_platform(Platform::Slack.as_str())
        .unwrap();
    let out = OutMessage {
        ctx: make_tenant_ctx("dev".into(), Some("acme".into()), None),
        tenant: "acme".into(),
        platform: Platform::Slack,
        chat_id: "C123".into(),
        thread_id: None,
        kind: OutKind::Text,
        text: Some("hi".into()),
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

    messaging_egress::process_message_internal(&out, &adapter, &bus, &cfg)
        .await
        .unwrap();

    let published = bus.take_published().await;
    assert_eq!(published.len(), 1);
    let (subject, payload) = &published[0];
    assert!(subject.contains("greentic.messaging.egress.out.acme.slack"));
    assert_eq!(payload["text"], "hi");
    assert_eq!(payload["adapter"], "slack-main");
}
