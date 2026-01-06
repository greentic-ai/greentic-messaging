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
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::fs::File;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use zip::write::SimpleFileOptions;

fn write_pack(path: &Path, flow_id: &str, component_id: &str) {
    let node_id = NodeId::new("start").unwrap();
    let component_id = ComponentId::new(component_id).unwrap();

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
                mapping: Value::Object(Default::default()),
            },
            output: OutputMapping {
                mapping: Value::Object(Default::default()),
            },
            routing: Routing::End,
            telemetry: TelemetryHints::default(),
        },
    );

    let mut entrypoints = BTreeMap::new();
    entrypoints.insert("default".to_string(), Value::String(node_id.to_string()));

    let flow = Flow {
        schema_version: "flow-v1".into(),
        id: FlowId::new(flow_id).unwrap(),
        kind: FlowKind::Messaging,
        entrypoints,
        nodes,
        metadata: FlowMetadata {
            title: Some("Smoke test flow".into()),
            description: None,
            tags: BTreeSet::new(),
            extra: Value::Null,
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
                    provider_type: "messaging.demo.provider".into(),
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

    let manifest = PackManifest {
        schema_version: "pack-v1".into(),
        pack_id: PackId::new("messaging.demo").unwrap(),
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
    zip.finish().expect("finish zip");
}

fn ensure_fixture_pack_with_component(
    name: &str,
    flow_id: &str,
    component_id: &str,
) -> std::path::PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("packs");
    fs::create_dir_all(&root).expect("create pack fixtures dir");
    let path = root.join(format!("{name}.gtpack"));
    if !path.exists() {
        write_pack(&path, flow_id, component_id);
    }
    path
}

fn ensure_fixture_pack(name: &str, flow_id: &str) -> std::path::PathBuf {
    ensure_fixture_pack_with_component(name, flow_id, "demo.component")
}

fn run_cli(args: &[&str]) -> String {
    run_cli_with_env(args, &[])
}

fn run_cli_with_env(args: &[&str], env: &[(&str, &str)]) -> String {
    let bin = env!("CARGO_BIN_EXE_greentic-messaging-test");
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root");
    let output = Command::new(bin)
        .current_dir(workspace_root)
        .envs(env.iter().cloned())
        .args(args)
        .output()
        .expect("run messaging-test CLI");
    if !output.status.success() {
        panic!(
            "command {:?} failed: stdout={}\nstderr={}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn packs_list_and_run() {
    let pack_path = ensure_fixture_pack("messaging-demo", "smoke");
    let pack_dir = pack_path.parent().unwrap();

    let list_out = run_cli(&["packs", "list", "--packs", pack_dir.to_str().unwrap()]);
    assert!(
        list_out.contains("messaging.demo"),
        "list output:\n{list_out}"
    );

    let run_out = run_cli(&[
        "packs",
        "run",
        pack_path.to_str().unwrap(),
        "--dry-run",
        "--no-resolve-components",
        "--env",
        "dev",
        "--tenant",
        "ci",
        "--team",
        "ci",
    ]);
    assert!(run_out.contains("result: ok"), "run output:\n{run_out}");
    assert!(
        run_out.contains("provider secrets"),
        "expected provider secrets section:\n{run_out}"
    );
}

#[test]
fn packs_all_scans_dir() {
    let pack_a = ensure_fixture_pack("messaging-a", "smoke");
    let _pack_b = ensure_fixture_pack("messaging-b", "smoke");
    let dir = pack_a.parent().unwrap();

    let out = run_cli(&[
        "packs",
        "all",
        "--packs",
        dir.to_str().unwrap(),
        "--dry-run",
        "--no-resolve-components",
    ]);
    assert!(
        out.contains("messaging.demo"),
        "expected packs to be reported:\n{out}"
    );
}

#[test]
fn packs_run_without_smoke_uses_first_flow() {
    let pack = ensure_fixture_pack("messaging-custom", "custom");
    let out = run_cli(&[
        "packs",
        "run",
        pack.to_str().unwrap(),
        "--dry-run",
        "--no-resolve-components",
    ]);
    assert!(
        out.contains("flow: custom"),
        "expected flow fallback to first flow:\n{out}"
    );
}

#[test]
fn packs_resolves_oci_component_via_distributor_client() {
    let pack_path = ensure_fixture_pack_with_component(
        "messaging-oci",
        "smoke",
        "greentic-ai.components.component-template",
    );
    let temp = tempfile::tempdir().expect("tempdir");
    let stub = temp.path().join("greentic-distributor-client");
    let mut script = File::create(&stub).expect("stub bin");
    writeln!(
        script,
        r#"#!/bin/sh
out_dir=""
while [ $# -gt 0 ]; do
  if [ "$1" = "--out" ]; then
    shift
    out_dir="$1"
  fi
  shift
done
mkdir -p "$out_dir/components"
exit 0
"#
    )
    .expect("write stub");
    drop(script);
    let mut perms = fs::metadata(&stub).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&stub, perms).unwrap();

    let out = run_cli_with_env(
        &[
            "packs",
            "run",
            pack_path.to_str().unwrap(),
            "--dry-run",
            "--env",
            "dev",
            "--tenant",
            "ci",
            "--team",
            "ci",
        ],
        &[
            ("GREENTIC_HOME", temp.path().to_str().unwrap()),
            ("GREENTIC_DISTRIBUTOR_CLIENT", stub.to_str().unwrap()),
        ],
    );

    assert!(
        out.contains("result: ok"),
        "expected pack validation to succeed with OCI component resolved:\n{out}"
    );
}
