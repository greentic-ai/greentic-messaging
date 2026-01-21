use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use greentic_types::component::{
    ComponentCapabilities, ComponentManifest, ComponentProfiles, ResourceHints,
};
use greentic_types::flow::ComponentRef;
use greentic_types::pack_manifest::{
    ExtensionInline, ExtensionRef, PackFlowEntry, PackKind, PackManifest, PackSignatures,
};
use greentic_types::provider::{
    PROVIDER_EXTENSION_ID, ProviderDecl, ProviderExtensionInline, ProviderRuntimeRef,
};
use greentic_types::{
    ComponentId, Flow, FlowId, FlowKind, FlowMetadata, InputMapping, Node, NodeId, OutputMapping,
    PackId, Routing, TelemetryHints,
};
use gsm_core::{InMemoryProviderInstallStore, ProviderInstallState, ProviderInstallStore};
use gsm_subscriptions_teams::{
    WorkerConfig, build_pack_index, discover_pack_files, run_sync_cycle,
};
use indexmap::IndexMap;
use semver::Version;
use serde_json::json;
use time::OffsetDateTime;

fn write_pack(path: &Path, provider_id: &str) {
    let node_id = NodeId::new("start").unwrap();
    let component_id = ComponentId::new("demo.component").unwrap();

    let mut nodes: IndexMap<NodeId, Node, greentic_types::flow::FlowHasher> = IndexMap::default();
    nodes.insert(
        node_id.clone(),
        Node {
            id: node_id.clone(),
            component: ComponentRef {
                id: component_id.clone(),
                pack_alias: None,
                operation: None,
            },
            input: InputMapping {
                mapping: serde_json::Value::Object(Default::default()),
            },
            output: OutputMapping {
                mapping: serde_json::Value::Object(Default::default()),
            },
            routing: Routing::End,
            telemetry: TelemetryHints::default(),
        },
    );

    let mut entrypoints = BTreeMap::new();
    entrypoints.insert(
        "default".to_string(),
        serde_json::Value::String(node_id.to_string()),
    );

    let flow = Flow {
        schema_version: "flow-v1".into(),
        id: FlowId::new("smoke").unwrap(),
        kind: FlowKind::Messaging,
        entrypoints,
        nodes,
        metadata: FlowMetadata {
            title: Some("E2E Smoke".into()),
            description: None,
            tags: BTreeSet::new(),
            extra: serde_json::Value::Null,
        },
    };

    let component = ComponentManifest {
        id: component_id.clone(),
        version: Version::new(0, 0, 1),
        supports: vec![FlowKind::Messaging],
        world: "greentic:provider/schema-core@1.0.0".into(),
        profiles: ComponentProfiles {
            default: Some("default".into()),
            supported: vec!["default".into()],
        },
        capabilities: ComponentCapabilities::default(),
        configurators: None,
        operations: Vec::new(),
        config_schema: None,
        resources: ResourceHints::default(),
        dev_flows: BTreeMap::new(),
    };

    let mut extensions = BTreeMap::new();
    extensions.insert(
        PROVIDER_EXTENSION_ID.to_string(),
        ExtensionRef {
            kind: PROVIDER_EXTENSION_ID.to_string(),
            version: "1.0.0".to_string(),
            digest: None,
            location: None,
            inline: Some(ExtensionInline::Provider(ProviderExtensionInline {
                providers: vec![ProviderDecl {
                    provider_type: provider_id.to_string(),
                    capabilities: vec!["messaging".into()],
                    ops: vec!["send".into()],
                    config_schema_ref: "schemas/config.json".into(),
                    state_schema_ref: None,
                    runtime: ProviderRuntimeRef {
                        component_ref: component_id.to_string(),
                        export: "schema-core-api".into(),
                        world: "greentic:provider/schema-core@1.0.0".into(),
                    },
                    docs_ref: None,
                }],
                additional_fields: BTreeMap::new(),
            })),
        },
    );
    extensions.insert(
        "messaging.subscriptions.v1".to_string(),
        ExtensionRef {
            kind: "messaging.subscriptions.v1".to_string(),
            version: "1.0.0".to_string(),
            digest: None,
            location: None,
            inline: Some(ExtensionInline::Other(json!({
                provider_id: {
                    "runtime": {
                        "component_ref": component_id.to_string(),
                        "export": "schema-core-api",
                        "world": "greentic:provider/schema-core@1.0.0"
                    },
                    "resources": []
                }
            }))),
        },
    );

    let manifest = PackManifest {
        schema_version: "pack-v1".into(),
        pack_id: PackId::new("messaging.teams").unwrap(),
        version: Version::new(0, 1, 0),
        kind: PackKind::Provider,
        publisher: "test".into(),
        components: vec![component],
        flows: vec![PackFlowEntry {
            id: flow.id.clone(),
            kind: flow.kind,
            flow,
            tags: Vec::new(),
            entrypoints: Vec::new(),
        }],
        dependencies: Vec::new(),
        capabilities: Vec::new(),
        secret_requirements: Vec::new(),
        signatures: PackSignatures::default(),
        bootstrap: None,
        extensions: Some(extensions),
    };

    let manifest_bytes = greentic_types::encode_pack_manifest(&manifest).unwrap();
    let file = fs::File::create(path).expect("pack file");
    let mut zip = zip::ZipWriter::new(file);
    let opts =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zip.start_file("manifest.cbor", opts)
        .expect("start manifest");
    zip.write_all(&manifest_bytes).expect("write manifest");
    zip.finish().expect("finish zip");
}

fn write_stub_provision(path: &Path, exit_ok: bool) {
    let script = if exit_ok {
        r#"#!/usr/bin/env bash
set -euo pipefail
echo '{"ok": true}'
"#
    } else {
        r#"#!/usr/bin/env bash
set -euo pipefail
echo "fail" >&2
exit 1
"#
    };
    fs::write(path, script).expect("write stub");
    let mut perms = fs::metadata(path).expect("stat stub").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).expect("chmod stub");
}

fn install_state(provider_id: &str) -> ProviderInstallState {
    let tenant = greentic_types::TenantCtx::new("dev".parse().unwrap(), "acme".parse().unwrap());
    let record = greentic_types::ProviderInstallRecord {
        tenant,
        provider_id: provider_id.to_string(),
        install_id: "install-a".parse().unwrap(),
        pack_id: "messaging.teams".parse().unwrap(),
        pack_version: Version::new(0, 1, 0),
        created_at: OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap(),
        updated_at: OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap(),
        config_refs: BTreeMap::new(),
        secret_refs: BTreeMap::new(),
        webhook_state: json!({}),
        subscriptions_state: json!({}),
        metadata: json!({}),
    };
    ProviderInstallState::new(record)
}

#[tokio::test]
async fn sync_cycle_updates_subscriptions_state() {
    let dir = tempfile::tempdir().expect("temp dir");
    let packs_root = dir.path().join("packs");
    fs::create_dir_all(&packs_root).unwrap();
    let pack_path = packs_root.join("messaging-teams.gtpack");
    write_pack(&pack_path, "messaging.teams");

    let provision_bin = dir.path().join("greentic-provision");
    write_stub_provision(&provision_bin, true);

    let store = InMemoryProviderInstallStore::default();
    store.insert(install_state("messaging.teams"));

    let pack_paths = discover_pack_files(&packs_root).unwrap();
    let extensions =
        gsm_core::load_provider_extensions_from_pack_files(&packs_root, &pack_paths).unwrap();
    let pack_index = build_pack_index(&pack_paths).unwrap();

    let config = WorkerConfig {
        env: "dev".parse().unwrap(),
        packs_root: packs_root.clone(),
        install_store_path: None,
        sync_interval: std::time::Duration::from_secs(1),
        provision_bin: provision_bin.to_string_lossy().to_string(),
        dry_run: true,
    };

    let mut failures: HashMap<
        gsm_subscriptions_teams::InstallKey,
        gsm_subscriptions_teams::FailureState,
    > = HashMap::new();
    let updated = run_sync_cycle(&store, &extensions, &pack_index, &config, &mut failures)
        .await
        .unwrap();
    assert_eq!(updated, 1);

    let tenant = greentic_types::TenantCtx::new("dev".parse().unwrap(), "acme".parse().unwrap());
    let state = store
        .get(&tenant, "messaging.teams", &"install-a".parse().unwrap())
        .expect("state");
    assert!(state.record.subscriptions_state.get("plan").is_some());
}

#[tokio::test]
async fn sync_cycle_marks_degraded_after_failures() {
    let dir = tempfile::tempdir().expect("temp dir");
    let packs_root = dir.path().join("packs");
    fs::create_dir_all(&packs_root).unwrap();
    let pack_path = packs_root.join("messaging-teams.gtpack");
    write_pack(&pack_path, "messaging.teams");

    let provision_bin = dir.path().join("greentic-provision");
    write_stub_provision(&provision_bin, false);

    let store = InMemoryProviderInstallStore::default();
    store.insert(install_state("messaging.teams"));

    let pack_paths = discover_pack_files(&packs_root).unwrap();
    let extensions =
        gsm_core::load_provider_extensions_from_pack_files(&packs_root, &pack_paths).unwrap();
    let pack_index = build_pack_index(&pack_paths).unwrap();

    let config = WorkerConfig {
        env: "dev".parse().unwrap(),
        packs_root: packs_root.clone(),
        install_store_path: None,
        sync_interval: std::time::Duration::from_secs(1),
        provision_bin: provision_bin.to_string_lossy().to_string(),
        dry_run: true,
    };

    let mut failures: HashMap<
        gsm_subscriptions_teams::InstallKey,
        gsm_subscriptions_teams::FailureState,
    > = HashMap::new();
    for _ in 0..3 {
        let _ = run_sync_cycle(&store, &extensions, &pack_index, &config, &mut failures).await;
    }

    let tenant = greentic_types::TenantCtx::new("dev".parse().unwrap(), "acme".parse().unwrap());
    let state = store
        .get(&tenant, "messaging.teams", &"install-a".parse().unwrap())
        .expect("state");
    assert_eq!(
        state
            .record
            .metadata
            .get("subscriptions_degraded")
            .and_then(|v| v.as_bool()),
        Some(true)
    );
}
