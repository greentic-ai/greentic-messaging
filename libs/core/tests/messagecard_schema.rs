#![cfg(feature = "adaptive-cards")]

use gsm_core::messaging_card::MessageCard;
use serde_json::json;

#[test]
fn round_trip_without_adaptive() {
    let card = MessageCard {
        title: Some("Schema".into()),
        text: Some("Hello schema".into()),
        footer: None,
        images: Vec::new(),
        actions: Vec::new(),
        allow_markdown: true,
        adaptive: None,
    };

    let value = serde_json::to_value(&card).expect("serialize");
    let recovered: MessageCard = serde_json::from_value(value).expect("round trip");
    assert!(recovered.allow_markdown);
    assert!(recovered.adaptive.is_none());
}

#[test]
fn round_trip_with_adaptive_payload() {
    let card = MessageCard {
        title: Some("Schema".into()),
        text: Some("Hello".into()),
        footer: None,
        images: Vec::new(),
        actions: Vec::new(),
        allow_markdown: true,
        adaptive: Some(json!({"type":"AdaptiveCard","version":"1.6","body":[]})),
    };

    let value = serde_json::to_value(&card).expect("serialize");
    let recovered: MessageCard = serde_json::from_value(value).expect("round trip");
    assert!(recovered.adaptive.is_some());
}
