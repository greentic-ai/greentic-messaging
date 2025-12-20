use gsm_core::render_plan::{RenderPlan, RenderTier, RenderWarning};
use serde_json::json;

#[test]
fn render_plan_serializes_and_roundtrips() {
    let plan = RenderPlan {
        tier: RenderTier::TierB,
        summary_text: Some("Summarized body".to_string()),
        actions: vec!["approve".into(), "reject".into()],
        attachments: vec!["https://example.com/attachment".into()],
        warnings: vec![RenderWarning {
            code: "text_truncated".into(),
            message: Some("Body trimmed to 256 chars".into()),
            path: Some("/body".into()),
        }],
        debug: Some(json!({
            "source": "unit_test",
            "meta": { "id": 42 }
        })),
    };

    let json_value = serde_json::to_value(&plan).expect("serialize");
    let expected = json!({
        "tier": "tier_b",
        "summary_text": "Summarized body",
        "actions": ["approve", "reject"],
        "attachments": ["https://example.com/attachment"],
        "warnings": [{
            "code": "text_truncated",
            "message": "Body trimmed to 256 chars",
            "path": "/body",
        }],
        "debug": {
            "source": "unit_test",
            "meta": { "id": 42 },
        },
    });
    assert_eq!(json_value, expected);

    let json_text = serde_json::to_string_pretty(&plan).expect("serialize to string");
    let roundtrip: RenderPlan = serde_json::from_str(&json_text).expect("deserialize");
    assert_eq!(roundtrip, plan);
}
