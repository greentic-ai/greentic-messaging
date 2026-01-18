use std::collections::BTreeMap;

use greentic_messaging_validate::messaging_validators;
use greentic_types::validate::{Diagnostic, Severity};
use greentic_types::{
    Flow, FlowId, FlowKind, PackFlowEntry, PackId, PackKind, PackManifest, PackSignatures,
    ProviderDecl, ProviderRuntimeRef,
};
use serde_json::Value;

fn base_manifest(pack_id: &str) -> PackManifest {
    PackManifest {
        schema_version: "1.0.0".to_string(),
        pack_id: pack_id.parse::<PackId>().unwrap(),
        version: semver::Version::new(0, 1, 0),
        kind: PackKind::Provider,
        publisher: "test".to_string(),
        components: Vec::new(),
        flows: Vec::new(),
        dependencies: Vec::new(),
        capabilities: Vec::new(),
        secret_requirements: Vec::new(),
        signatures: PackSignatures::default(),
        bootstrap: None,
        extensions: None,
    }
}

fn add_provider(manifest: &mut PackManifest, provider_type: &str, ops: &[&str], schema_ref: &str) {
    let provider = ProviderDecl {
        provider_type: provider_type.to_string(),
        capabilities: Vec::new(),
        ops: ops.iter().map(|op| (*op).to_string()).collect(),
        config_schema_ref: schema_ref.to_string(),
        state_schema_ref: None,
        runtime: ProviderRuntimeRef {
            component_ref: "provider".to_string(),
            export: "run".to_string(),
            world: "messaging".to_string(),
        },
        docs_ref: None,
    };
    manifest
        .ensure_provider_extension_inline()
        .providers
        .push(provider);
}

fn flow_entry(flow_id: &str, entrypoints: &[&str]) -> PackFlowEntry {
    let flow_id = flow_id.parse::<FlowId>().unwrap();
    let mut entrypoint_map = BTreeMap::new();
    for entry in entrypoints {
        entrypoint_map.insert((*entry).to_string(), Value::Null);
    }

    PackFlowEntry {
        id: flow_id.clone(),
        kind: FlowKind::Messaging,
        flow: Flow {
            schema_version: "1.0.0".to_string(),
            id: flow_id,
            kind: FlowKind::Messaging,
            entrypoints: entrypoint_map,
            nodes: Default::default(),
            metadata: Default::default(),
        },
        tags: Vec::new(),
        entrypoints: entrypoints
            .iter()
            .map(|entry| (*entry).to_string())
            .collect(),
    }
}

fn collect_diagnostics(manifest: &PackManifest) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for validator in messaging_validators() {
        if validator.applies(manifest) {
            diagnostics.extend(validator.validate(manifest));
        }
    }
    diagnostics
}

#[test]
fn non_messaging_pack_has_no_diagnostics() {
    let manifest = base_manifest("utility-pack");
    let diagnostics = collect_diagnostics(&manifest);
    assert!(diagnostics.is_empty());
}

#[test]
fn messaging_pack_requires_provider_decls() {
    let manifest = base_manifest("messaging-test");
    let diagnostics = collect_diagnostics(&manifest);
    assert!(diagnostics.iter().any(|diag| {
        diag.code == "MSG_NO_PROVIDER_DECL" && matches!(diag.severity, Severity::Error)
    }));
}

#[test]
fn setup_declared_without_entry_emits_error() {
    let mut manifest = base_manifest("messaging-test");
    manifest.flows.push(flow_entry("setup-bootstrap", &[]));
    let diagnostics = collect_diagnostics(&manifest);
    assert!(diagnostics.iter().any(|diag| {
        diag.code == "MSG_SETUP_ENTRY_MISSING" && matches!(diag.severity, Severity::Error)
    }));
}

#[test]
fn setup_flow_without_public_url_schema_warns() {
    let mut manifest = base_manifest("messaging-test");
    add_provider(
        &mut manifest,
        "messaging.test",
        &["send"],
        "schemas/messaging/test/config.schema.json",
    );
    manifest.flows.push(flow_entry("setup", &["setup"]));
    let diagnostics = collect_diagnostics(&manifest);
    assert!(diagnostics.iter().any(|diag| {
        diag.code == "MSG_SETUP_PUBLIC_URL_NOT_ASSERTED" && matches!(diag.severity, Severity::Warn)
    }));
}

#[test]
fn subscriptions_flow_suppresses_missing_subscriptions_warning() {
    let mut manifest = base_manifest("messaging-test");
    add_provider(
        &mut manifest,
        "messaging.test",
        &["send"],
        "schemas/messaging/test/config.schema.json",
    );
    manifest.flows.push(flow_entry("sync-subscriptions", &[]));
    let diagnostics = collect_diagnostics(&manifest);
    assert!(
        !diagnostics
            .iter()
            .any(|diag| diag.code == "MSG_SUBSCRIPTIONS_DECLARED_BUT_NO_FLOW")
    );
}
