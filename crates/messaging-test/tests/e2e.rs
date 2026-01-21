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
use indexmap::IndexMap;
use semver::Version;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::fs::File;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use zip::write::SimpleFileOptions;

fn write_pack(path: &Path) {
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

    let flow = Flow {
        schema_version: "flow-v1".into(),
        id: FlowId::new("smoke").unwrap(),
        kind: FlowKind::Messaging,
        entrypoints: BTreeMap::new(),
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
                    provider_type: "messaging.e2e.provider".into(),
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
        "messaging.oauth.v1".to_string(),
        ExtensionRef {
            kind: "messaging.oauth.v1".to_string(),
            version: "1.0.0".to_string(),
            digest: None,
            location: None,
            inline: Some(ExtensionInline::Other(json!({
                "e2e-provider": {
                    "provider": "e2e",
                    "scopes": []
                }
            }))),
        },
    );

    let manifest = PackManifest {
        schema_version: "pack-v1".into(),
        pack_id: PackId::new("messaging.e2e").unwrap(),
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
    let file = File::create(path).expect("pack file");
    let mut zip = zip::ZipWriter::new(file);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zip.start_file("manifest.cbor", opts)
        .expect("start manifest");
    zip.write_all(&manifest_bytes).expect("write manifest");
    zip.add_directory("flows/smoke", opts)
        .expect("add flow dir");
    zip.start_file("flows/smoke/flow.ygtc", opts)
        .expect("start flow");
    let flow_yaml = r#"id: smoke
type: messaging
in: start
nodes:
  start:
    routes: []
"#;
    zip.write_all(flow_yaml.as_bytes()).expect("write flow");
    zip.finish().expect("finish zip");
}

#[test]
fn e2e_dry_run_passes() {
    let root = tempfile::tempdir().expect("temp dir");
    let pack_path = root.path().join("messaging-e2e.gtpack");
    write_pack(&pack_path);
    let provision_path = root.path().join("greentic-provision");
    write_stub_provision(&provision_path);

    let bin = env!("CARGO_BIN_EXE_greentic-messaging-test");
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root");
    let report_path = root.path().join("report.json");

    let output = Command::new(bin)
        .current_dir(workspace_root)
        .env("GREENTIC_PROVISION_CLI", &provision_path)
        .args([
            "e2e",
            "--packs",
            root.path().to_str().unwrap(),
            "--dry-run",
            "--report",
            report_path.to_str().unwrap(),
        ])
        .output()
        .expect("run e2e");
    if !output.status.success() {
        let report = fs::read_to_string(&report_path).unwrap_or_else(|_| "<missing>".into());
        panic!(
            "e2e failed: stdout={}\nstderr={}\nreport={report}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let report = fs::read_to_string(report_path).expect("read report");
    assert!(
        report.contains("\"status\": \"pass\""),
        "unexpected report:\n{report}"
    );
}

fn write_stub_provision(path: &Path) {
    let script = r#"#!/usr/bin/env bash
set -euo pipefail
action="${1:-}"
if [[ "$action" == "setup" ]]; then
  echo '{"operations":[{"type":"webhook","detail":"stub"}]}'
  exit 0
fi
if [[ "$action" == "sync-subscriptions" ]]; then
  echo '{"operations":[{"type":"subscription","detail":"stub"}]}'
  exit 0
fi
echo '{"operations":[]}'
"#;
    fs::write(path, script).expect("write provision stub");
    let mut perms = fs::metadata(path)
        .expect("stat provision stub")
        .permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).expect("chmod provision stub");
}
