use std::str::FromStr;
use std::sync::Arc;

use gsm_bus::InMemoryBusClient;
use gsm_core::{
    AdapterDescriptor, ChannelMessage, DefaultAdapterPacksConfig, InMemoryProviderInstallStore,
    LoggingRunnerClient, MessagingAdapterKind, OutKind, OutMessage, Platform,
    ProviderExtensionsRegistry, ProviderInstallState, ProviderInstallStore, make_tenant_ctx,
};
use gsm_egress::adapter_registry::AdapterLookup;
use gsm_egress::process_message_internal;
use gsm_gateway::config::GatewayConfig;
use gsm_gateway::http::{GatewayState, NormalizedRequest, handle_ingress};
use gsm_gateway::load_adapter_registry;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn test_gateway_config() -> GatewayConfig {
    GatewayConfig {
        env: "dev".try_into().unwrap(),
        nats_url: "nats://localhost".into(),
        addr: "127.0.0.1:0".parse().unwrap(),
        default_team: "default".into(),
        subject_prefix: gsm_core::INGRESS_SUBJECT_PREFIX.to_string(),
        worker_routing: None,
        worker_routes: std::collections::BTreeMap::new(),
        packs_root: PathBuf::from("packs"),
        default_packs: DefaultAdapterPacksConfig::default(),
        extra_pack_paths: Vec::new(),
        install_store_path: None,
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

fn test_install_record(
    install_id: &str,
    platform: &str,
    channel_id: &str,
) -> greentic_types::ProviderInstallRecord {
    use greentic_types::{EnvId, PackId, ProviderInstallId, TeamId, TenantCtx, TenantId};
    use semver::Version;
    use time::OffsetDateTime;

    let tenant = TenantCtx::new(
        "dev".parse::<EnvId>().expect("env"),
        "acme".parse::<TenantId>().expect("tenant"),
    )
    .with_team(Some("team".parse::<TeamId>().expect("team")));
    let mut config_refs = BTreeMap::new();
    config_refs.insert("config".to_string(), "state:config".to_string());
    let mut secret_refs = BTreeMap::new();
    secret_refs.insert("token".to_string(), "secrets:token".to_string());
    let metadata = serde_json::json!({
        "routing": {
            "platform": platform,
            "channel_id": channel_id
        }
    });
    greentic_types::ProviderInstallRecord {
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
        metadata,
    }
}

#[tokio::test]
async fn ingress_to_egress_round_trip_over_in_memory_bus() {
    let bus = Arc::new(InMemoryBusClient::default());
    let adapters = load_adapter_registry(
        Path::new("packs"),
        &DefaultAdapterPacksConfig::default(),
        &[],
    );
    let install_store = Arc::new(InMemoryProviderInstallStore::default());
    let record = test_install_record("install-a", "slack", "workspace-1");
    let mut install_state = ProviderInstallState::new(record);
    install_state
        .secrets
        .insert("token".into(), "secret".into());
    install_state
        .config
        .insert("config".into(), serde_json::json!({"ok": true}));
    install_store.insert(install_state);
    let state = Arc::new(GatewayState {
        bus: bus.clone(),
        config: test_gateway_config(),
        adapters,
        provider_extensions: ProviderExtensionsRegistry::default(),
        install_store,
        workers: std::collections::BTreeMap::new(),
        worker_default: None,
    });

    let payload = NormalizedRequest {
        provider_id: Some("messaging.slack".into()),
        provider_channel_id: Some("workspace-1".into()),
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
        platform: Platform::from_str(channel.channel_id.as_str()).unwrap(),
        chat_id: channel.session_id.clone(),
        thread_id: None,
        kind: OutKind::Text,
        text: Some(text.to_string()),
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
    let record = test_install_record("install-a", "slack", "workspace-1");
    let mut install_state = ProviderInstallState::new(record);
    install_state
        .secrets
        .insert("token".into(), "secret".into());
    install_state
        .config
        .insert("config".into(), serde_json::json!({"ok": true}));

    process_message_internal(&out, &adapter, bus.as_ref(), &runner, &cfg, &install_state)
        .await
        .unwrap();

    let published = bus.take_published().await;
    assert_eq!(published.len(), 1);
    let (egress_subject, payload) = &published[0];
    assert!(egress_subject.contains("greentic.messaging.egress.dev.acme.team.slack"));
    assert_eq!(payload["text"], text);
    assert_eq!(payload["adapter"], "slack-main");
}
