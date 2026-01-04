use greentic_pack::builder::{PackMeta, Signing};
use greentic_pack::messaging::{MessagingAdapter, MessagingAdapterKind, MessagingSection};
use greentic_types::flow::FlowHasher;
use greentic_types::pack_manifest::{
    ExtensionInline, ExtensionRef, PackFlowEntry, PackKind, PackManifest, PackSignatures,
};
use greentic_types::provider::{
    PROVIDER_EXTENSION_ID, ProviderDecl, ProviderExtensionInline, ProviderRuntimeRef,
};
use greentic_types::{Flow, FlowId, FlowKind, FlowMetadata, PackId};
use indexmap::IndexMap;
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::Write;
use std::process::Command;
use tempfile::NamedTempFile;

const DRY_ENV: &str = "GREENTIC_MESSAGING_CLI_DRY_RUN";

fn cli_cmd() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_greentic-messaging"));
    cmd.env(DRY_ENV, "1");
    cmd
}

fn run_and_capture(args: &[&str]) -> String {
    let mut cmd = cli_cmd();
    cmd.args(args);
    let output = cmd.output().expect("run greentic-messaging CLI");
    if !output.status.success() {
        panic!(
            "CLI command {:?} failed: status={:?}\nstdout={}\nstderr={}",
            args,
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn serve_ingress_slack_dry_run() {
    let stdout = run_and_capture(&["serve", "ingress", "slack", "--tenant", "acme"]);
    assert!(
        stdout.contains("cargo run -p gsm-gateway"),
        "stdout did not contain dry-run marker:\n{}",
        stdout
    );
}

#[test]
fn serve_ingress_with_pack_sets_env() {
    let tmp = NamedTempFile::new().unwrap();
    let stdout = run_and_capture(&[
        "serve",
        "ingress",
        "webchat",
        "--tenant",
        "acme",
        "--pack",
        tmp.path().to_str().unwrap(),
    ]);
    assert!(
        stdout.contains("MESSAGING_ADAPTER_PACK_PATHS"),
        "stdout did not include pack env:\n{}",
        stdout
    );
    assert!(
        stdout.contains("cargo run -p gsm-gateway"),
        "stdout did not contain gateway run:\n{}",
        stdout
    );
}

#[test]
fn flows_run_dry_run() {
    let tmp = NamedTempFile::new().unwrap();
    let stdout = run_and_capture(&[
        "flows",
        "run",
        "--flow",
        tmp.path().to_str().unwrap(),
        "--platform",
        "slack",
        "--tenant",
        "acme",
    ]);
    assert!(
        stdout.contains("dry-run) make run-runner"),
        "stdout did not contain dry-run marker:\n{}",
        stdout
    );
}

#[test]
fn messaging_test_wrapper_dry_run() {
    let stdout = run_and_capture(&["test", "fixtures"]);
    assert!(
        stdout.contains("dry-run) cargo run -p greentic-messaging-test"),
        "stdout did not contain dry-run marker:\n{}",
        stdout
    );
}

#[test]
fn dev_down_dry_run() {
    let stdout = run_and_capture(&["dev", "down"]);
    assert!(
        stdout.contains("dry-run) make stack-down"),
        "stdout did not contain stack-down marker:\n{}",
        stdout
    );
}

#[test]
fn info_lists_adapters_from_pack() {
    let pack = NamedTempFile::new().unwrap();
    std::fs::write(
        pack.path(),
        r#"
id: info-pack
version: 0.0.1
messaging:
  adapters:
    - name: info-ingress
      kind: ingress
      component: info@0.0.1
    - name: info-egress
      kind: egress
      component: info@0.0.1
    - name: info-both
      kind: ingress-egress
      component: info@0.0.1
"#,
    )
    .unwrap();

    let stdout = run_and_capture(&[
        "info",
        "--pack",
        pack.path().to_str().unwrap(),
        "--no-default-packs",
    ]);
    assert!(
        stdout.contains("info-ingress")
            && stdout.contains("info-egress")
            && stdout.contains("info-both"),
        "stdout did not list adapters:\n{}",
        stdout
    );
}

fn write_provider_pack(path: &std::path::Path) {
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
                    provider_type: "messaging.slack.bot".into(),
                    capabilities: vec!["messaging".into()],
                    ops: vec!["send".into()],
                    config_schema_ref: "schemas/config.json".into(),
                    state_schema_ref: None,
                    runtime: ProviderRuntimeRef {
                        component_ref: "slack-adapter@1.0.0".into(),
                        export: "schema-core-api".into(),
                        world: "greentic:provider/schema-core@1.0.0".into(),
                    },
                    docs_ref: None,
                }],
                additional_fields: BTreeMap::new(),
            })),
        },
    );

    let flow = Flow {
        schema_version: "messaging".into(),
        id: FlowId::new("provider-flow").unwrap(),
        kind: FlowKind::Messaging,
        entrypoints: BTreeMap::new(),
        nodes: IndexMap::with_hasher(FlowHasher::default()),
        metadata: FlowMetadata {
            title: Some("Provider flow".into()),
            description: None,
            tags: Default::default(),
            extra: Value::Null,
        },
    };

    let manifest = PackManifest {
        schema_version: "pack-v1".into(),
        pack_id: PackId::new("cli.providers").unwrap(),
        version: semver::Version::new(0, 0, 1),
        kind: PackKind::Provider,
        publisher: "cli-test".into(),
        components: Vec::new(),
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
    let file = std::fs::File::create(path).expect("pack file");
    let mut zip = zip::ZipWriter::new(file);
    let opts =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zip.start_file("manifest.cbor", opts)
        .expect("start manifest");
    zip.write_all(&manifest_bytes).expect("write manifest");
    zip.finish().expect("finish zip");
}

#[test]
fn info_lists_adapters_from_provider_extension_pack() {
    let dir = tempfile::tempdir().unwrap();
    let pack_path = dir.path().join("provider.gtpack");
    write_provider_pack(&pack_path);

    let stdout = run_and_capture(&[
        "info",
        "--pack",
        pack_path.to_str().unwrap(),
        "--no-default-packs",
    ]);
    assert!(
        stdout.contains("messaging.slack.bot"),
        "stdout did not list provider-derived adapter:\n{}",
        stdout
    );
    assert!(
        stdout.contains("provider-flow") && stdout.contains("kind=Messaging"),
        "stdout did not list provider flows:\n{}",
        stdout
    );
}

fn write_provider_pack_with_flow_hints(path: &std::path::Path) {
    use greentic_types::flow::FlowMetadata;
    use greentic_types::pack_manifest::{
        ExtensionInline, ExtensionRef, PackKind, PackManifest, PackSignatures,
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
                    provider_type: "messaging.slack.api".into(),
                    capabilities: vec!["messaging".into()],
                    ops: vec!["send".into()],
                    config_schema_ref: "schemas/config.json".into(),
                    state_schema_ref: None,
                    runtime: ProviderRuntimeRef {
                        component_ref: "slack-adapter@1.0.0".into(),
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
        "messaging.provider_flow_hints".to_string(),
        ExtensionRef {
            kind: "messaging.provider_flow_hints".to_string(),
            version: "1.0.0".to_string(),
            digest: None,
            location: None,
            inline: Some(ExtensionInline::Other(serde_json::json!({
                "messaging.slack.api": {
                    "setup_default": "hint-setup",
                    "diagnostics": "hint-missing"
                }
            }))),
        },
    );

    let flow = Flow {
        schema_version: "messaging".into(),
        id: FlowId::new("hint-setup").unwrap(),
        kind: FlowKind::Messaging,
        entrypoints: BTreeMap::new(),
        nodes: IndexMap::with_hasher(FlowHasher::default()),
        metadata: FlowMetadata {
            title: Some("Setup flow".into()),
            description: None,
            tags: Default::default(),
            extra: Value::Null,
        },
    };

    let manifest = PackManifest {
        schema_version: "pack-v1".into(),
        pack_id: PackId::new("cli.provider.hints").unwrap(),
        version: semver::Version::new(0, 0, 1),
        kind: PackKind::Provider,
        publisher: "cli-test".into(),
        components: Vec::new(),
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
    let file = std::fs::File::create(path).expect("pack file");
    let mut zip = zip::ZipWriter::new(file);
    let opts =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zip.start_file("manifest.cbor", opts)
        .expect("start manifest");
    zip.write_all(&manifest_bytes).expect("write manifest");
    zip.finish().expect("finish zip");
}

#[test]
fn info_lists_provider_flow_hints_and_missing_annotation() {
    let dir = tempfile::tempdir().unwrap();
    let pack_path = dir.path().join("provider-hints.gtpack");
    write_provider_pack_with_flow_hints(&pack_path);

    let stdout = run_and_capture(&[
        "info",
        "--pack",
        pack_path.to_str().unwrap(),
        "--no-default-packs",
    ]);
    assert!(
        stdout.contains("messaging.slack.api"),
        "stdout did not list provider adapter:\n{}",
        stdout
    );
    assert!(
        stdout.contains("provider flows (from pack cli.provider.hints):"),
        "stdout did not list provider flow hint source:\n{}",
        stdout
    );
    assert!(
        stdout.contains("setup_default     : hint-setup"),
        "stdout did not show setup_default hint:\n{}",
        stdout
    );
    assert!(
        stdout.contains("diagnostics       : hint-missing (missing)"),
        "stdout did not annotate missing flow id:\n{}",
        stdout
    );
}

fn write_legacy_pack(path: &std::path::Path) {
    let flow_yaml = r#"id: legacy-flow
type: messaging
in: start
nodes:
  start:
    routes: []
"#;
    let flow_bundle = greentic_flow::flow_bundle::FlowBundle {
        id: "legacy-flow".to_string(),
        kind: "messaging".to_string(),
        entry: "start".to_string(),
        yaml: flow_yaml.to_string(),
        json: serde_json::json!({
            "id": "legacy-flow",
            "type": "messaging",
            "in": "start",
            "nodes": { "start": { "routes": [] } }
        }),
        hash_blake3: greentic_flow::flow_bundle::blake3_hex(flow_yaml),
        nodes: Vec::new(),
    };
    let meta = PackMeta {
        pack_version: greentic_pack::builder::PACK_VERSION,
        pack_id: "cli.legacy".to_string(),
        version: semver::Version::new(0, 0, 1),
        name: "legacy-pack".to_string(),
        kind: None,
        description: None,
        authors: Vec::new(),
        license: None,
        homepage: None,
        support: None,
        vendor: None,
        imports: Vec::new(),
        entry_flows: vec![flow_bundle.id.clone()],
        created_at_utc: "1970-01-01T00:00:00Z".to_string(),
        events: None,
        repo: None,
        messaging: Some(MessagingSection {
            adapters: Some(vec![MessagingAdapter {
                name: "legacy-main".into(),
                kind: MessagingAdapterKind::IngressEgress,
                component: "legacy-comp@1.0.0".into(),
                default_flow: Some("flows/messaging/legacy/default.ygtc".into()),
                custom_flow: None,
                capabilities: None,
            }]),
        }),
        interfaces: Vec::new(),
        annotations: serde_json::Map::new(),
        distribution: None,
        components: Vec::new(),
    };
    let wasm_path = path.with_extension("wasm");
    std::fs::write(&wasm_path, b"00").expect("write wasm stub");
    greentic_pack::builder::PackBuilder::new(meta)
        .with_flow(flow_bundle)
        .with_component(greentic_pack::builder::ComponentArtifact {
            name: "legacy-comp".to_string(),
            version: semver::Version::new(0, 0, 1),
            wasm_path,
            schema_json: None,
            manifest_json: None,
            capabilities: None,
            world: None,
            hash_blake3: None,
        })
        .with_signing(Signing::Dev)
        .build(path)
        .expect("build legacy pack");
}

#[test]
fn info_lists_flows_and_legacy_adapter_flow_hint() {
    let dir = tempfile::tempdir().unwrap();
    let pack_path = dir.path().join("legacy.gtpack");
    write_legacy_pack(&pack_path);

    let stdout = run_and_capture(&[
        "info",
        "--pack",
        pack_path.to_str().unwrap(),
        "--no-default-packs",
    ]);
    assert!(
        stdout.contains("legacy-main"),
        "stdout did not list legacy adapter:\n{}",
        stdout
    );
    assert!(
        stdout.contains("flow=flows/messaging/legacy/default.ygtc"),
        "stdout did not show legacy flow hint:\n{}",
        stdout
    );
    assert!(
        stdout.contains("legacy-flow") && stdout.to_lowercase().contains("kind=messaging"),
        "stdout did not list legacy flow:\n{}",
        stdout
    );
}

#[test]
fn admin_wrappers_dry_run() {
    let slack = run_and_capture(&[
        "admin",
        "slack",
        "oauth-helper",
        "--",
        "--listen",
        "0.0.0.0:8085",
    ]);
    assert!(
        slack.contains("dry-run) cargo run -p gsm-slack-oauth"),
        "stdout did not contain dry-run marker:\n{}",
        slack
    );

    let teams = run_and_capture(&[
        "admin",
        "teams",
        "setup",
        "--",
        "--tenant",
        "t",
        "--client-id",
        "c",
        "--client-secret",
        "s",
        "--chat-id",
        "chat",
    ]);
    assert!(
        teams.contains("dry-run) cargo run --manifest-path scripts/Cargo.toml --bin teams_setup"),
        "stdout did not contain teams setup marker:\n{}",
        teams
    );

    let telegram = run_and_capture(&["admin", "telegram", "setup"]);
    assert!(
        telegram
            .contains("dry-run) cargo run --manifest-path scripts/Cargo.toml --bin telegram_setup"),
        "stdout did not contain telegram setup marker:\n{}",
        telegram
    );

    let whatsapp = run_and_capture(&["admin", "whatsapp", "setup"]);
    assert!(
        whatsapp
            .contains("dry-run) cargo run --manifest-path scripts/Cargo.toml --bin whatsapp_setup"),
        "stdout did not contain whatsapp setup marker:\n{}",
        whatsapp
    );
}
