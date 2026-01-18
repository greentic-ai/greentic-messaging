use greentic_types::PackId;
use greentic_types::pack_manifest::{
    ExtensionInline, ExtensionRef, PackKind, PackManifest, PackSignatures,
};
use gsm_core::{
    load_provider_extensions_from_pack_files,
    pack_extensions::INGRESS_EXTENSION_ID,
    provider_capabilities::ProviderCapabilitiesV1,
    provider_registry::{CapsSource, ProviderCapsRegistry},
};
use std::io::Write;
use tempfile::TempDir;

#[test]
fn register_and_fetch_caps() {
    let mut registry = ProviderCapsRegistry::new();
    let caps = ProviderCapabilitiesV1 {
        supports_markdown: true,
        ..Default::default()
    };
    registry.register_provider(
        "provider-1",
        "1.0.0",
        CapsSource::FromPackManifest,
        caps.clone(),
        Some("encoder://v1".into()),
    );

    let fetched = registry.get_caps("provider-1").expect("caps present");
    assert_eq!(fetched, &caps);

    // Unknown provider returns None.
    assert!(registry.get_caps("missing").is_none());
}

#[test]
fn records_encoder_and_source() {
    let mut registry = ProviderCapsRegistry::new();
    let caps = ProviderCapabilitiesV1::default();
    registry.register_provider(
        "provider-2",
        "2.0.0",
        CapsSource::Override,
        caps.clone(),
        None,
    );

    let record = registry.get("provider-2").expect("record exists");
    assert_eq!(record.version, "2.0.0");
    assert_eq!(record.caps_source, CapsSource::Override);
    assert_eq!(record.encoder_ref, None);
}

#[test]
fn loads_provider_extensions_from_pack() {
    let temp = TempDir::new().expect("temp dir");
    let gtpack_path = temp.path().join("provider.gtpack");

    let extensions = std::collections::BTreeMap::from([(
        INGRESS_EXTENSION_ID.to_string(),
        ExtensionRef {
            kind: INGRESS_EXTENSION_ID.to_string(),
            version: "1.0.0".into(),
            digest: None,
            location: None,
            inline: Some(ExtensionInline::Other(serde_json::json!({
                "messaging.slack.bot": {
                    "runtime": {
                        "component_ref": "slack-adapter@1.0.0",
                        "export": "run",
                        "world": "greentic:provider/schema-core@1.0.0"
                    },
                    "capabilities": {
                        "supports_webhook_validation": true,
                        "content_types": ["application/json"]
                    }
                }
            }))),
        },
    )]);

    let manifest = PackManifest {
        schema_version: "pack-v1".into(),
        pack_id: PackId::new("provider.extensions").unwrap(),
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
    let file = std::fs::File::create(&gtpack_path).expect("pack file");
    let mut zip = zip::ZipWriter::new(file);
    let opts =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zip.start_file("manifest.cbor", opts)
        .expect("start manifest");
    zip.write_all(&manifest_bytes).expect("write manifest");
    zip.finish().expect("finish zip");

    let registry =
        load_provider_extensions_from_pack_files(temp.path(), std::slice::from_ref(&gtpack_path))
            .expect("load provider extensions");
    let provider = registry
        .ingress
        .get("messaging.slack.bot")
        .expect("ingress provider registered");
    assert_eq!(provider.runtime.component_ref, "slack-adapter@1.0.0");
    assert!(provider.capabilities.supports_webhook_validation);
}
