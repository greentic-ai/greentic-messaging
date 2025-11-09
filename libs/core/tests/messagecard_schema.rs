#![cfg(feature = "adaptive-cards")]

use gsm_core::messaging_card::{MessageCard, MessageCardKind, OauthCard, OauthProvider};
use serde_json::json;

#[test]
fn round_trip_without_adaptive() {
    let card = MessageCard {
        kind: MessageCardKind::Standard,
        title: Some("Schema".into()),
        text: Some("Hello schema".into()),
        footer: None,
        images: Vec::new(),
        actions: Vec::new(),
        allow_markdown: true,
        adaptive: None,
        oauth: None,
    };

    let value = serde_json::to_value(&card).expect("serialize");
    let recovered: MessageCard = serde_json::from_value(value).expect("round trip");
    assert!(recovered.allow_markdown);
    assert!(recovered.adaptive.is_none());
}

#[test]
fn round_trip_with_adaptive_payload() {
    let card = MessageCard {
        kind: MessageCardKind::Standard,
        title: Some("Schema".into()),
        text: Some("Hello".into()),
        footer: None,
        images: Vec::new(),
        actions: Vec::new(),
        allow_markdown: true,
        adaptive: Some(json!({"type":"AdaptiveCard","version":"1.6","body":[]})),
        oauth: None,
    };

    let value = serde_json::to_value(&card).expect("serialize");
    let recovered: MessageCard = serde_json::from_value(value).expect("round trip");
    assert!(recovered.adaptive.is_some());
}

#[test]
fn round_trip_with_oauth_payload() {
    let card = MessageCard {
        kind: MessageCardKind::Oauth,
        title: Some("Sign in".into()),
        text: Some("Please continue".into()),
        footer: Some("Powered by Greentic".into()),
        images: Vec::new(),
        actions: Vec::new(),
        allow_markdown: true,
        adaptive: None,
        oauth: Some(OauthCard {
            provider: OauthProvider::Microsoft,
            scopes: vec!["User.Read".into()],
            resource: None,
            prompt: None,
            start_url: Some("https://oauth/start".into()),
            connection_name: Some("m365".into()),
            metadata: Some(json!({"tenant": "acme"})),
        }),
    };

    let value = serde_json::to_value(&card).expect("serialize");
    let recovered: MessageCard = serde_json::from_value(value).expect("round trip");
    assert!(matches!(recovered.kind, MessageCardKind::Oauth));
    assert!(recovered.oauth.is_some());
}
