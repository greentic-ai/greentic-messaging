use gsm_core::{
    DefaultAdapterPacksConfig, InMemoryProviderInstallStore, Platform, ProviderExtensionsRegistry,
    ProviderInstallState, ProviderInstallStore,
};
use gsm_gateway::InMemoryBusClient;
use gsm_gateway::config::GatewayConfig;
use gsm_gateway::http::{GatewayState, NormalizedRequest, handle_ingress};
use gsm_gateway::load_adapter_registry;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn test_config() -> GatewayConfig {
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

fn load_default_adapters() -> gsm_core::AdapterRegistry {
    load_adapter_registry(
        Path::new("packs"),
        &DefaultAdapterPacksConfig::default(),
        &[],
    )
}

fn install_store_with_slack() -> Arc<dyn ProviderInstallStore> {
    let store = Arc::new(InMemoryProviderInstallStore::default());
    let record = test_install_record("install-a", "slack", "workspace-1");
    let mut state = ProviderInstallState::new(record);
    state.secrets.insert("token".into(), "secret".into());
    state
        .config
        .insert("config".into(), serde_json::json!({"ok": true}));
    store.insert(state);
    store
}

fn install_store_missing_secret() -> Arc<dyn ProviderInstallStore> {
    let store = Arc::new(InMemoryProviderInstallStore::default());
    let record = test_install_record("install-missing-secret", "slack", "workspace-1");
    let mut state = ProviderInstallState::new(record);
    state
        .config
        .insert("config".into(), serde_json::json!({"ok": true}));
    store.insert(state);
    store
}

fn install_store_missing_config() -> Arc<dyn ProviderInstallStore> {
    let store = Arc::new(InMemoryProviderInstallStore::default());
    let record = test_install_record("install-missing-config", "slack", "workspace-1");
    let mut state = ProviderInstallState::new(record);
    state.secrets.insert("token".into(), "secret".into());
    store.insert(state);
    store
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
async fn ingress_is_normalized_and_published() {
    let bus = Arc::new(InMemoryBusClient::default());
    let adapters = load_default_adapters();
    let install_store = install_store_with_slack();
    let state = Arc::new(GatewayState {
        bus: bus.clone(),
        config: test_config(),
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
    let adapters = load_default_adapters();
    let config = test_config();
    let install_store = install_store_with_slack();
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
        provider_extensions: ProviderExtensionsRegistry::default(),
        install_store,
        workers,
        worker_default: Some(worker_cfg),
    });

    let payload = NormalizedRequest {
        provider_id: Some("messaging.slack".into()),
        provider_channel_id: Some("workspace-1".into()),
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
    let adapters = load_default_adapters();
    let config = test_config();
    let worker_cfg = gsm_core::WorkerRoutingConfig::default();
    let install_store = install_store_with_slack();
    let mut workers = std::collections::BTreeMap::new();
    workers.insert(
        worker_cfg.worker_id.clone(),
        Arc::new(FailingWorker) as Arc<dyn gsm_core::WorkerClient>,
    );

    let state = Arc::new(GatewayState {
        bus: bus.clone(),
        config,
        adapters,
        provider_extensions: ProviderExtensionsRegistry::default(),
        install_store,
        workers,
        worker_default: Some(worker_cfg),
    });

    let payload = NormalizedRequest {
        provider_id: Some("messaging.slack".into()),
        provider_channel_id: Some("workspace-1".into()),
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
    let adapters = load_default_adapters();
    let config = test_config();
    let worker_config = Some(gsm_core::WorkerRoutingConfig::default());
    let install_store = install_store_with_slack();
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
        provider_extensions: ProviderExtensionsRegistry::default(),
        install_store,
        workers: {
            let mut map = std::collections::BTreeMap::new();
            map.insert(
                worker_config.as_ref().unwrap().worker_id.clone(),
                Arc::new(worker) as Arc<dyn gsm_core::WorkerClient>,
            );
            map
        },
        worker_default: worker_config.clone(),
    });

    let payload = NormalizedRequest {
        provider_id: Some("messaging.slack".into()),
        provider_channel_id: Some("workspace-1".into()),
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

#[tokio::test]
async fn ingress_routes_between_multiple_installs() {
    let bus = Arc::new(InMemoryBusClient::default());
    let adapters = load_default_adapters();
    let store = Arc::new(InMemoryProviderInstallStore::default());
    for (install_id, channel_id) in [("install-a", "workspace-1"), ("install-b", "workspace-2")] {
        let record = test_install_record(install_id, "slack", channel_id);
        let mut state = ProviderInstallState::new(record);
        state.secrets.insert("token".into(), "secret".into());
        state
            .config
            .insert("config".into(), serde_json::json!({"ok": true}));
        store.insert(state);
    }

    let state = Arc::new(GatewayState {
        bus: bus.clone(),
        config: test_config(),
        adapters,
        provider_extensions: ProviderExtensionsRegistry::default(),
        install_store: store,
        workers: std::collections::BTreeMap::new(),
        worker_default: None,
    });

    let payload = NormalizedRequest {
        provider_id: Some("messaging.slack".into()),
        provider_channel_id: Some("workspace-2".into()),
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
    let (_subject, raw) = &published[0];
    let msg: gsm_core::ChannelMessage = serde_json::from_value(raw.clone()).unwrap();
    assert_eq!(
        msg.payload["metadata"]["install_id"],
        serde_json::json!("install-b")
    );
}

#[tokio::test]
async fn ingress_returns_error_for_missing_install() {
    let bus = Arc::new(InMemoryBusClient::default());
    let adapters = load_default_adapters();
    let state = Arc::new(GatewayState {
        bus: bus.clone(),
        config: test_config(),
        adapters,
        provider_extensions: ProviderExtensionsRegistry::default(),
        install_store: Arc::new(InMemoryProviderInstallStore::default()),
        workers: std::collections::BTreeMap::new(),
        worker_default: None,
    });

    let payload = NormalizedRequest {
        provider_id: Some("messaging.slack".into()),
        install_id: Some("install-missing".into()),
        chat_id: Some("chat-1".into()),
        user_id: Some("user-1".into()),
        text: Some("hi".into()),
        ..Default::default()
    };

    let err = handle_ingress(
        "acme".into(),
        Some("team".into()),
        "slack".into(),
        state,
        payload,
        Default::default(),
    )
    .await
    .unwrap_err();

    assert_eq!(err.0, axum::http::StatusCode::NOT_FOUND);
    assert_eq!(err.1.0.code(), Some("install_not_found"));
}

#[tokio::test]
async fn ingress_returns_error_for_missing_secret() {
    let bus = Arc::new(InMemoryBusClient::default());
    let adapters = load_default_adapters();
    let state = Arc::new(GatewayState {
        bus: bus.clone(),
        config: test_config(),
        adapters,
        provider_extensions: ProviderExtensionsRegistry::default(),
        install_store: install_store_missing_secret(),
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

    let err = handle_ingress(
        "acme".into(),
        Some("team".into()),
        "slack".into(),
        state,
        payload,
        Default::default(),
    )
    .await
    .unwrap_err();

    assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
    assert_eq!(err.1.0.code(), Some("missing_secret"));
}

#[tokio::test]
async fn ingress_returns_error_for_missing_config() {
    let bus = Arc::new(InMemoryBusClient::default());
    let adapters = load_default_adapters();
    let state = Arc::new(GatewayState {
        bus: bus.clone(),
        config: test_config(),
        adapters,
        provider_extensions: ProviderExtensionsRegistry::default(),
        install_store: install_store_missing_config(),
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

    let err = handle_ingress(
        "acme".into(),
        Some("team".into()),
        "slack".into(),
        state,
        payload,
        Default::default(),
    )
    .await
    .unwrap_err();

    assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
    assert_eq!(err.1.0.code(), Some("missing_config"));
}
