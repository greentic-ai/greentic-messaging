use gsm_core::provider_capabilities::{
    CapabilitiesError, ProviderCapabilitiesV1, ProviderLimitsV1,
};

#[test]
fn defaults_are_conservative() {
    let caps = ProviderCapabilitiesV1::default();
    assert_eq!(caps.version, "v1");
    assert!(!caps.supports_adaptive_cards);
    assert!(!caps.supports_markdown);
    assert!(!caps.supports_html);
    assert!(!caps.supports_images);
    assert!(!caps.supports_buttons);
    assert!(!caps.supports_threads);
    assert_eq!(caps.limits, ProviderLimitsV1::default());
}

#[test]
fn serde_roundtrip() {
    let caps = ProviderCapabilitiesV1 {
        version: "v1".into(),
        supports_adaptive_cards: true,
        supports_markdown: true,
        supports_html: false,
        supports_images: true,
        supports_buttons: true,
        supports_threads: false,
        max_text_len: Some(4096),
        max_payload_bytes: Some(1024 * 64),
        max_actions: Some(5),
        max_buttons_per_row: Some(5),
        max_total_buttons: Some(25),
        limits: ProviderLimitsV1 {
            max_text_len: Some(2048),
            max_payload_bytes: Some(32 * 1024),
            max_actions: Some(4),
            max_buttons_per_row: Some(4),
            max_total_buttons: Some(10),
        },
    };
    let json = serde_json::to_string(&caps).expect("serialize");
    let back: ProviderCapabilitiesV1 = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(caps, back);
}

#[test]
fn validation_rejects_bad_version() {
    let caps = ProviderCapabilitiesV1 {
        version: "v2".into(),
        ..Default::default()
    };
    assert_eq!(caps.validate(), Err(CapabilitiesError::BadVersion));
}

#[test]
fn validation_rejects_buttons_row_gt_total() {
    let caps = ProviderCapabilitiesV1 {
        max_buttons_per_row: Some(3),
        max_total_buttons: Some(2),
        ..Default::default()
    };
    assert_eq!(
        caps.validate(),
        Err(CapabilitiesError::ButtonsRowExceedsTotal)
    );

    let caps2 = ProviderCapabilitiesV1 {
        limits: ProviderLimitsV1 {
            max_buttons_per_row: Some(4),
            max_total_buttons: Some(3),
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(
        caps2.validate(),
        Err(CapabilitiesError::ButtonsRowExceedsTotal)
    );
}

#[test]
fn validation_accepts_ok_capabilities() {
    let caps = ProviderCapabilitiesV1 {
        supports_markdown: true,
        supports_threads: true,
        max_buttons_per_row: Some(3),
        max_total_buttons: Some(5),
        ..Default::default()
    };
    assert_eq!(caps.validate(), Ok(()));
}
