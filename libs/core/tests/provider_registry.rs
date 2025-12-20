use gsm_core::{
    provider_capabilities::ProviderCapabilitiesV1,
    provider_registry::{CapsSource, ProviderCapsRegistry},
};

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
