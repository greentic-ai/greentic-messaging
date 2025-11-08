#![cfg(feature = "adaptive-cards")]

use gsm_core::messaging_card::ir::{InputChoice, InputKind};
use gsm_core::messaging_card::tier::Tier;
use gsm_core::messaging_card::{MessageCardIr, MessageCardIrBuilder, SlackRenderer};
use gsm_core::{AppLink, PlatformRenderer};
use serde_json::{Value, json};

#[test]
fn slack_basic_blocks_snapshot() {
    let renderer = SlackRenderer::default();
    let ir = sample_ir(false);
    let rendered = renderer.render(&ir);
    assert!(!rendered.used_modal);
    assert_eq!(rendered.payload, load_fixture("slack/basic.json"));
}

#[test]
fn slack_modal_snapshot() {
    let renderer = SlackRenderer::default();
    let ir = sample_ir(true);
    let rendered = renderer.render(&ir);
    assert!(rendered.used_modal);
    assert_eq!(rendered.payload, load_fixture("slack/modal.json"));
}

fn sample_ir(include_input: bool) -> MessageCardIr {
    let mut builder = MessageCardIrBuilder::default()
        .tier(Tier::Premium)
        .title("Slack Snapshot")
        .primary_text("Body text", true)
        .image(
            "https://example.com/banner.png".into(),
            Some("Banner".into()),
        )
        .fact("Status", "Green")
        .open_url("Docs", "https://example.com/docs")
        .postback("Ack", json!({"ok": true}));

    if include_input {
        builder = builder.input(
            Some("Choose".into()),
            InputKind::Choice,
            Some("choice".into()),
            vec![
                InputChoice {
                    title: "Yes".into(),
                    value: "yes".into(),
                },
                InputChoice {
                    title: "No".into(),
                    value: "no".into(),
                },
            ],
        );
    }

    let mut ir = builder.build();
    ir.head.footer = Some("Footer".into());
    ir.meta.app_link = Some(AppLink {
        base_url: "https://premium.example/deeplink".into(),
        secret: Some("secret-token".into()),
        tenant: None,
        scope: None,
    });
    ir
}

fn load_fixture(path: &str) -> Value {
    let base = format!("tests/fixtures/renderers/{path}");
    let data = std::fs::read_to_string(base).expect("fixture missing");
    serde_json::from_str(&data).expect("invalid json")
}
