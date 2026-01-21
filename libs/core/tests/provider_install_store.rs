use std::collections::BTreeMap;

use greentic_types::{
    EnvId, PackId, ProviderInstallId, ProviderInstallRecord, TenantCtx, TenantId,
};
use gsm_core::{
    ProviderInstallState, ProviderInstallStore, ProviderInstallStoreSnapshot,
    load_install_store_from_path,
};
use semver::Version;
use time::OffsetDateTime;

fn record(install_id: &str) -> ProviderInstallRecord {
    let tenant = TenantCtx::new(
        "dev".parse::<EnvId>().expect("env"),
        "acme".parse::<TenantId>().expect("tenant"),
    );
    ProviderInstallRecord {
        tenant,
        provider_id: "messaging.slack".to_string(),
        install_id: install_id.parse::<ProviderInstallId>().expect("install id"),
        pack_id: "messaging-slack".parse::<PackId>().expect("pack id"),
        pack_version: Version::parse("1.0.0").expect("version"),
        created_at: OffsetDateTime::from_unix_timestamp(1_700_000_000).expect("created_at"),
        updated_at: OffsetDateTime::from_unix_timestamp(1_700_000_100).expect("updated_at"),
        config_refs: BTreeMap::new(),
        secret_refs: BTreeMap::new(),
        webhook_state: serde_json::json!({}),
        subscriptions_state: serde_json::json!({}),
        metadata: serde_json::json!({}),
    }
}

#[test]
fn load_install_store_from_records() {
    let dir = tempfile::tempdir().expect("temp dir");
    let snapshot = ProviderInstallStoreSnapshot {
        records: vec![record("install-a")],
        states: Vec::new(),
    };
    let path = dir.path().join("installs.json");
    std::fs::write(&path, serde_json::to_string_pretty(&snapshot).unwrap())
        .expect("write snapshot");

    let store = load_install_store_from_path(&path).expect("load store");
    let tenant = TenantCtx::new(
        "dev".parse::<EnvId>().expect("env"),
        "acme".parse::<TenantId>().expect("tenant"),
    );
    let install_id = "install-a"
        .parse::<ProviderInstallId>()
        .expect("install id");
    let state = store
        .get(&tenant, "messaging.slack", &install_id)
        .expect("state");
    assert_eq!(state.record.install_id.as_str(), "install-a");
}

#[test]
fn load_install_store_from_states() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut state = ProviderInstallState::new(record("install-b"));
    state
        .config
        .insert("config".into(), serde_json::json!({"ok": true}));
    state.secrets.insert("token".into(), "secret".into());
    let snapshot = ProviderInstallStoreSnapshot {
        records: Vec::new(),
        states: vec![state.clone()],
    };
    let path = dir.path().join("installs.json");
    std::fs::write(&path, serde_json::to_string_pretty(&snapshot).unwrap())
        .expect("write snapshot");

    let store = load_install_store_from_path(&path).expect("load store");
    let tenant = TenantCtx::new(
        "dev".parse::<EnvId>().expect("env"),
        "acme".parse::<TenantId>().expect("tenant"),
    );
    let install_id = "install-b"
        .parse::<ProviderInstallId>()
        .expect("install id");
    let resolved = store
        .get(&tenant, "messaging.slack", &install_id)
        .expect("state");
    assert_eq!(resolved.config.get("config"), state.config.get("config"));
    assert_eq!(resolved.secrets.get("token"), state.secrets.get("token"));
}
