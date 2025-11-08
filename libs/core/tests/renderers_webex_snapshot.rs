#![cfg(feature = "adaptive-cards")]

use gsm_core::messaging_card::ir::{InputChoice, InputKind};
use gsm_core::messaging_card::tier::Tier;
use gsm_core::messaging_card::{MessageCardIr, MessageCardIrBuilder, WebexRenderer};
use gsm_core::{AppLink, PlatformRenderer};
use serde_json::{Value, json};

#[test]
fn webex_basic_snapshot() {
    let renderer = WebexRenderer::default();
    let ir = sample_ir(false);
    let rendered = renderer.render(&ir);
    assert_eq!(rendered.payload, load_fixture("webex/basic.json"));
    assert!(!rendered.used_modal);
    assert!(
        rendered
            .warnings
            .iter()
            .any(|w| w == "webex.factset_downgraded")
    );
}

#[test]
fn webex_interactive_snapshot_downgrades_inputs() {
    let renderer = WebexRenderer::default();
    let ir = sample_ir(true);
    let rendered = renderer.render(&ir);
    assert_eq!(rendered.payload, load_fixture("webex/interactive.json"));
    assert!(
        rendered
            .warnings
            .iter()
            .any(|w| w == "webex.inputs_not_supported")
    );
}

fn sample_ir(include_input: bool) -> MessageCardIr {
    let mut builder = MessageCardIrBuilder::default()
        .tier(Tier::Premium)
        .title("Webex Snapshot")
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
    ir.head.text = Some("Subtitle text".into());
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
