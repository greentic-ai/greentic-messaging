#![cfg(feature = "adaptive-cards")]

use gsm_core::adaptivecards::{CanonicalizeError, canonicalize, stable_json, validate};
use serde_json::json;

#[test]
fn orders_body_and_actions_stably() {
    let card = json!({
        "type": "AdaptiveCard",
        "version": "1.2",
        "body": [
            { "type": "TextBlock", "text": "beta" },
            { "type": "TextBlock", "text": "alpha" }
        ],
        "actions": [
            { "type": "Action.Submit", "title": "second" },
            { "type": "Action.Submit", "title": "first" }
        ]
    });

    let canonical = canonicalize(card).expect("canonicalize");
    let stable = stable_json(&canonical);
    let body = stable
        .get("body")
        .and_then(|v| v.as_array())
        .expect("body array");
    assert_eq!(body[0]["text"], "alpha");
    assert_eq!(body[1]["text"], "beta");

    let actions = stable
        .get("actions")
        .and_then(|v| v.as_array())
        .expect("actions array");
    assert_eq!(actions[0]["title"], "first");
    assert_eq!(actions[1]["title"], "second");
}

#[test]
fn trims_text_blocks_and_defaults_wrap() {
    let card = json!({
        "type": "AdaptiveCard",
        "version": "1.6",
        "body": [
            { "type": "TextBlock", "text": "  hello\n" }
        ]
    });

    let canonical = canonicalize(card).expect("canonicalize");
    let stable = stable_json(&canonical);
    let body = stable.get("body").and_then(|v| v.as_array()).unwrap();
    assert_eq!(body[0]["text"], "hello");
    assert_eq!(body[0]["wrap"], true);
}

#[test]
fn validate_requires_body() {
    let card = canonicalize(json!({
        "type": "AdaptiveCard",
        "version": "1.6",
        "actions": []
    }))
    .expect("canonicalize");

    let err = validate(&card).expect_err("missing body rejected");
    assert!(matches!(err, CanonicalizeError::MissingBody));
}

#[test]
fn stable_json_is_idempotent() {
    let card = canonicalize(json!({
        "type": "AdaptiveCard",
        "version": "1.4",
        "body": [
            { "type": "TextBlock", "text": "A" },
            { "type": "Image", "url": "https://example.com" }
        ],
        "actions": []
    }))
    .expect("canonicalize");

    let first = stable_json(&card);
    let roundtrip = canonicalize(first.clone()).expect("roundtrip canonicalize");
    let second = stable_json(&roundtrip);
    assert_eq!(first, second);
}

#[cfg(feature = "proptest")]
mod prop {
    use super::*;
    use proptest::collection::vec;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn canonicalization_is_idempotent(texts in vec(".*", 0..6)) {
            let body: Vec<_> = texts.into_iter().map(|text| {
                json!({"type": "TextBlock", "text": text})
            }).collect();
            let card_value = json!({
                "type": "AdaptiveCard",
                "version": "1.6",
                "body": body,
                "actions": []
            });

            let canonical = canonicalize(card_value).expect("canonicalize");
            let stable_once = stable_json(&canonical);
            let canonical_again = canonicalize(stable_once.clone()).expect("canonicalize again");
            let stable_twice = stable_json(&canonical_again);

            prop_assert_eq!(stable_once, stable_twice);
        }
    }
}
