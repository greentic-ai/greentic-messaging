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

fn run_and_capture_with_dir(args: &[&str], dir: &std::path::Path) -> String {
    let mut cmd = cli_cmd();
    cmd.current_dir(dir).args(args);
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
fn messaging_test_wrapper_dry_run() {
    let stdout = run_and_capture(&["test", "fixtures"]);
    assert!(
        stdout.contains("dry-run) cargo run -p greentic-messaging-test"),
        "stdout did not contain dry-run marker:\n{}",
        stdout
    );
}

#[test]
fn dev_up_dry_run() {
    let stdout = run_and_capture(&["dev", "up"]);
    assert!(
        stdout.contains("cargo run -p gsm-gateway"),
        "stdout did not contain gateway run:\n{}",
        stdout
    );
    assert!(
        stdout.contains("cargo run -p gsm-runner"),
        "stdout did not contain runner run:\n{}",
        stdout
    );
    assert!(
        stdout.contains("cargo run -p gsm-egress"),
        "stdout did not contain egress run:\n{}",
        stdout
    );
}

#[test]
fn info_lists_adapters_from_pack() {
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
fn dev_setup_writes_install_record_in_dry_run() {
    let dir = tempfile::tempdir().unwrap();
    let pack_path = dir.path().join("provider.gtpack");
    write_provider_pack(&pack_path);

    let _ = run_and_capture_with_dir(
        &[
            "dev",
            "up",
            "--pack",
            pack_path.to_str().unwrap(),
            "--no-default-packs",
            "--tunnel",
            "none",
        ],
        dir.path(),
    );

    let _ = run_and_capture_with_dir(
        &[
            "dev",
            "setup",
            "messaging.slack.bot",
            "--pack",
            pack_path.to_str().unwrap(),
            "--no-default-packs",
        ],
        dir.path(),
    );

    let installs_path = dir.path().join(".greentic/dev/installs.json");
    let payload = std::fs::read_to_string(&installs_path).expect("install record written");
    let store: serde_json::Value = serde_json::from_str(&payload).expect("valid json");
    let records = store
        .get("states")
        .and_then(|v| v.as_array())
        .or_else(|| store.get("records").and_then(|v| v.as_array()))
        .expect("records array");
    assert!(
        records.iter().any(|r| {
            let provider_id = r
                .get("record")
                .and_then(|rec| rec.get("provider_id"))
                .or_else(|| r.get("provider_id"))
                .and_then(|v| v.as_str());
            provider_id == Some("messaging.slack.bot")
        }),
        "install record missing: {payload}"
    );
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

#[test]
fn admin_slack_wrapper_dry_run() {
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
}
