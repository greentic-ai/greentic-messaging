use gsm_core::{
    AdapterDescriptor, LoggingRunnerClient, MessagingAdapterKind, OutKind, OutMessage, Platform,
    ProviderInstallState, make_tenant_ctx,
};
use gsm_egress::InMemoryBusClient;
use gsm_egress::adapter_registry::AdapterLookup;
use std::collections::BTreeMap;

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
    let mut meta = BTreeMap::new();
    meta.insert(
        gsm_core::PROVIDER_ID_KEY.to_string(),
        serde_json::json!("messaging.slack"),
    );
    meta.insert(
        gsm_core::INSTALL_ID_KEY.to_string(),
        serde_json::json!("install-a"),
    );
    let out = OutMessage {
        ctx: make_tenant_ctx("acme".into(), Some("team".into()), None),
        tenant: "acme".into(),
        platform: Platform::Slack,
        chat_id: "C123".into(),
        thread_id: None,
        kind: OutKind::Text,
        text: Some("hi".into()),
        message_card: None,
        adaptive_card: None,
        meta,
    };

    let cfg = gsm_egress::config::EgressConfig {
        env: "dev".try_into().unwrap(),
        nats_url: "nats://localhost".into(),
        subject_filter: "greentic.messaging.egress.dev.>".into(),
        adapter: None,
        packs_root: "packs".into(),
        egress_prefix: gsm_core::EGRESS_SUBJECT_PREFIX.to_string(),
        runner_http_url: None,
        runner_http_api_key: None,
        install_store_path: None,
    };

    let runner = LoggingRunnerClient;
    let install_state = install_state("install-a");

    gsm_egress::process_message_internal(&out, &adapter, &bus, &runner, &cfg, &install_state)
        .await
        .unwrap();

    let published = bus.take_published().await;
    assert_eq!(published.len(), 1);
    let (subject, payload) = &published[0];
    assert!(subject.contains("greentic.messaging.egress.dev.acme.team.slack"));
    assert_eq!(payload["text"], "hi");
    assert_eq!(payload["adapter"], "slack-main");
}

fn install_state(install_id: &str) -> ProviderInstallState {
    use greentic_types::{
        EnvId, PackId, ProviderInstallId, ProviderInstallRecord, TenantCtx, TenantId,
    };
    use semver::Version;
    use time::OffsetDateTime;

    let tenant = TenantCtx::new(
        "dev".parse::<EnvId>().expect("env"),
        "acme".parse::<TenantId>().expect("tenant"),
    );
    let mut config_refs = BTreeMap::new();
    config_refs.insert("config".to_string(), "state:config".to_string());
    let mut secret_refs = BTreeMap::new();
    secret_refs.insert("token".to_string(), "secrets:token".to_string());
    let record = ProviderInstallRecord {
        tenant,
        provider_id: "messaging.slack".to_string(),
        install_id: install_id.parse::<ProviderInstallId>().expect("install id"),
        pack_id: "messaging-slack".parse::<PackId>().expect("pack id"),
        pack_version: Version::parse("1.0.0").expect("version"),
        created_at: OffsetDateTime::from_unix_timestamp(1_700_000_000).expect("created_at"),
        updated_at: OffsetDateTime::from_unix_timestamp(1_700_000_100).expect("updated_at"),
        config_refs,
        secret_refs,
        webhook_state: serde_json::json!({}),
        subscriptions_state: serde_json::json!({}),
        metadata: serde_json::json!({}),
    };
    let mut state = ProviderInstallState::new(record);
    state.secrets.insert("token".into(), "secret".into());
    state
        .config
        .insert("config".into(), serde_json::json!({"ok": true}));
    state
}
