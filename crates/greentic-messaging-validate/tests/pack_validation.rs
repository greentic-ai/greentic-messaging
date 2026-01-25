use std::collections::BTreeMap;
use std::env;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use greentic_types::{
    encode_pack_manifest,
    pack_manifest::{ExtensionInline, ExtensionRef, PackKind, PackManifest, PackSignatures},
    provider::{
        ProviderDecl, ProviderExtensionInline, ProviderRuntimeRef, PROVIDER_EXTENSION_ID,
    },
    validate::Severity,
    PackId,
};
use semver::Version;
use serde::Deserialize;
use tempfile::tempdir;
use zip::write::FileOptions;
use zip::ZipWriter;
use zip::CompressionMethod;

#[test]
fn minimal_manifest_reports_no_errors() {
    let manifest = base_provider_manifest();
    let dir = tempdir().expect("allocate pack dir");
    let pack_path = dir.path().join("minimal.gtpack");
    write_manifest_gtpack(&pack_path, &manifest);

    let validator_pack = ensure_validator_pack();
    let validation = run_doctor_for_pack(&pack_path, &validator_pack);
    assert!(!validation.has_errors);
    assert!(
        validation
            .diagnostics
            .iter()
            .all(|diag| !matches!(diag.severity, Severity::Error)),
        "unexpected error diagnostics: {:?}",
        validation.diagnostics
    );
}

#[test]
fn missing_provider_declaration_reports_error() {
    let mut manifest = base_provider_manifest();
    manifest.extensions = None;

    let dir = tempdir().expect("allocate pack dir");
    let pack_path = dir.path().join("missing-provider.gtpack");
    write_manifest_gtpack(&pack_path, &manifest);

    let validator_pack = ensure_validator_pack();
    let validation = run_doctor_for_pack(&pack_path, &validator_pack);
    assert!(
        validation.has_errors,
        "doctor should mark packs with no providers as erroneous"
    );
    assert!(
        find_diagnostic(&validation.diagnostics, "MSG_NO_PROVIDER_DECL")
            .is_some_and(|diag| matches!(diag.severity, Severity::Error)),
        "expected MSG_NO_PROVIDER_DECL error, saw {:?}",
        validation.diagnostics
    );
}

#[test]
fn missing_secret_requirements_warns() {
    let manifest = base_provider_manifest();
    let dir = tempdir().expect("allocate pack dir");
    let pack_path = dir.path().join("missing-secrets.gtpack");
    write_manifest_gtpack(&pack_path, &manifest);

    let validator_pack = ensure_validator_pack();
    let validation = run_doctor_for_pack(&pack_path, &validator_pack);
    let warning = find_diagnostic(&validation.diagnostics, "MSG_SECRETS_REQUIREMENTS_NOT_DISCOVERABLE")
        .expect("expected secret requirements warning");
    assert!(
        matches!(warning.severity, Severity::Warn),
        "unexpected severity for MSG_SECRETS_REQUIREMENTS_NOT_DISCOVERABLE: {:?}",
        warning
    );
}

#[test]
fn cbor_only_pack_validates() {
    let manifest = base_provider_manifest();
    let dir = tempdir().expect("allocate pack dir");
    let pack_path = dir.path().join("cbor-only.gtpack");
    write_manifest_gtpack(&pack_path, &manifest);
    assert_cbor_only_pack(&pack_path);

    let validator_pack = ensure_validator_pack();
    let validation = run_doctor_for_pack(&pack_path, &validator_pack);
    assert!(!validation.has_errors, "CBOR-only pack should not error: {:?}", validation);
}

fn base_provider_manifest() -> PackManifest {
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
                    provider_type: "messaging.test.provider".into(),
                    capabilities: vec!["messaging".into()],
                    ops: vec!["send".into()],
                    config_schema_ref: "schemas/config.json".into(),
                    state_schema_ref: None,
                    runtime: ProviderRuntimeRef {
                        component_ref: "messaging.test/component@1.0.0".into(),
                        export: "schema-core-api".into(),
                        world: "greentic:provider/schema-core@1.0.0".into(),
                    },
                    docs_ref: None,
                }],
                additional_fields: BTreeMap::new(),
            })),
        },
    );

    PackManifest {
        schema_version: "pack-v1".into(),
        pack_id: PackId::new("messaging.test.pack").expect("valid pack id"),
        version: Version::new(0, 1, 0),
        kind: PackKind::Provider,
        publisher: "messaging-test".into(),
        components: Vec::new(),
        flows: Vec::new(),
        dependencies: Vec::new(),
        capabilities: Vec::new(),
        secret_requirements: Vec::new(),
        signatures: PackSignatures::default(),
        bootstrap: None,
        extensions: Some(extensions),
    }
}

fn write_manifest_gtpack(path: &Path, manifest: &PackManifest) {
    let manifest_bytes = encode_pack_manifest(manifest).expect("encode manifest");
    let file = File::create(path).expect("create pack file");
    let mut zip = ZipWriter::new(file);
    let opts = FileOptions::default().compression_method(CompressionMethod::Stored);
    zip.start_file("manifest.cbor", opts)
        .expect("start manifest");
    zip.write_all(&manifest_bytes).expect("write manifest");
    zip.finish().expect("finish pack");
}

fn ensure_validator_pack() -> PathBuf {
    let repo_root = repo_root();
    let validator_pack = repo_root.join("dist").join("validators-messaging.gtpack");
    if !validator_pack.exists() {
        let status = Command::new("bash")
            .arg("scripts/build-validator-pack.sh")
            .current_dir(&repo_root)
            .status()
            .expect("build validator pack");
        assert!(status.success(), "build-validator-pack.sh failed");
    }
    validator_pack
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("resolve repo root")
        .to_path_buf()
}

fn run_doctor_for_pack(pack: &Path, validator_pack: &Path) -> DoctorValidation {
    let output = Command::new("greentic-pack")
        .args([
            "doctor",
            "--format",
            "json",
            "--validate",
            "--allow-oci-tags",
            "--offline",
            "--pack",
            pack.to_str().expect("pack path"),
            "--validator-pack",
            validator_pack.to_str().expect("validator pack"),
        ])
        .output()
        .expect("run greentic-pack doctor");
    parse_doctor_output(&String::from_utf8_lossy(&output.stdout))
}

fn parse_doctor_output(stdout: &str) -> DoctorValidation {
    #[derive(Deserialize)]
    struct DoctorOutput {
        #[serde(default)]
        validation: Option<DoctorValidation>,
    }

    let parsed: DoctorOutput = serde_json::from_str(stdout)
        .expect("greentic-pack doctor emits JSON validation output");
    parsed.validation.unwrap_or_default()
}

fn find_diagnostic<'a>(diags: &'a [DoctorDiagnostic], code: &str) -> Option<&'a DoctorDiagnostic> {
    diags.iter().find(|diag| diag.code == code)
}

fn assert_cbor_only_pack(path: &Path) {
    let file = File::open(path).expect("open pack");
    let mut archive = zip::ZipArchive::new(file).expect("zip archive");
    assert_eq!(archive.len(), 1, "pack should contain only manifest");
    let entry = archive.by_index(0).expect("manifest entry");
    assert_eq!(entry.name(), "manifest.cbor");
}

#[derive(Debug, Deserialize)]
struct DoctorDiagnostic {
    severity: Severity,
    code: String,
    message: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    hint: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct DoctorValidation {
    #[serde(default)]
    diagnostics: Vec<DoctorDiagnostic>,
    #[serde(default)]
    has_errors: bool,
}
