use std::io::Write;

use greentic_types::PackId;
use greentic_types::pack_manifest::{
    ExtensionInline, ExtensionRef, PackKind, PackManifest, PackSignatures,
};
use greentic_types::provider::{
    PROVIDER_EXTENSION_ID, ProviderDecl, ProviderExtensionInline, ProviderRuntimeRef,
};
use gsm_core::AdapterRegistry;

fn write_provider_gtpack(path: &std::path::Path, provider_type: &str, component_ref: &str) {
    let mut extensions = std::collections::BTreeMap::new();
    extensions.insert(
        PROVIDER_EXTENSION_ID.to_string(),
        ExtensionRef {
            kind: PROVIDER_EXTENSION_ID.to_string(),
            version: "1.0.0".to_string(),
            digest: None,
            location: None,
            inline: Some(ExtensionInline::Provider(ProviderExtensionInline {
                providers: vec![ProviderDecl {
                    provider_type: provider_type.to_string(),
                    capabilities: vec![],
                    ops: vec![],
                    config_schema_ref: "schemas/config.json".into(),
                    state_schema_ref: None,
                    runtime: ProviderRuntimeRef {
                        component_ref: component_ref.to_string(),
                        export: "run".into(),
                        world: "greentic:provider/schema-core@1.0.0".into(),
                    },
                    docs_ref: None,
                }],
                additional_fields: Default::default(),
            })),
        },
    );

    let manifest = PackManifest {
        schema_version: "pack-v1".into(),
        pack_id: PackId::new("egress.providers.test").unwrap(),
        version: semver::Version::new(0, 0, 1),
        kind: PackKind::Provider,
        publisher: "test".into(),
        components: Vec::new(),
        flows: Vec::new(),
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
fn loads_adapter_from_provider_extension_pack() {
    let dir = tempfile::tempdir().unwrap();
    let gtpack_path = dir.path().join("provider.gtpack");
    write_provider_gtpack(&gtpack_path, "slack-main", "slack-adapter@1.0.0");

    let registry = AdapterRegistry::load_from_paths(dir.path(), std::slice::from_ref(&gtpack_path))
        .expect("load adapters");
    let adapter = registry.get("slack-main").expect("adapter registered");
    assert_eq!(adapter.component, "slack-adapter@1.0.0");
}
