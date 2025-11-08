#![cfg(feature = "adaptive-cards")]

use gsm_core::messaging_card::ir::{InputChoice, InputKind, IrAction};
use gsm_core::messaging_card::tier::Tier;
use gsm_core::messaging_card::{MessageCardIr, MessageCardIrBuilder, WhatsAppRenderer};
use gsm_core::{AppLink, PlatformRenderer};
use serde_json::{Value, json};

#[test]
fn whatsapp_basic_snapshot() {
    let renderer = WhatsAppRenderer::default();
    let ir = sample_ir(false, 2);
    let rendered = renderer.render(&ir);
    assert_eq!(rendered.payload, load_fixture("whatsapp/basic.json"));
    assert!(rendered.warnings.is_empty());
}

#[test]
fn whatsapp_trims_buttons_and_warns() {
    let renderer = WhatsAppRenderer::default();
    let ir = sample_ir(false, 5);
    let rendered = renderer.render(&ir);
    assert_eq!(
        rendered.payload,
        load_fixture("whatsapp/interactive_trimmed.json")
    );
    assert!(
        rendered
            .warnings
            .iter()
            .any(|w| w == "whatsapp.actions_truncated")
    );
}

#[test]
fn whatsapp_inputs_downgraded() {
    let renderer = WhatsAppRenderer::default();
    let ir = sample_ir(true, 2);
    let rendered = renderer.render(&ir);
    assert_eq!(
        rendered.payload,
        load_fixture("whatsapp/inputs_downgraded.json")
    );
    assert!(
        rendered
            .warnings
            .iter()
            .any(|w| w == "whatsapp.inputs_not_supported")
    );
}

fn sample_ir(include_input: bool, action_count: usize) -> MessageCardIr {
    let mut builder = MessageCardIrBuilder::default()
        .tier(Tier::Premium)
        .title("WhatsApp Snapshot")
        .primary_text("Body text", true)
        .image(
            "https://example.com/banner.png".into(),
            Some("Banner".into()),
        )
        .fact("Status", "Green");

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
    ir.actions.clear();
    let base_actions = [
        IrAction::OpenUrl {
            title: "Docs".into(),
            url: "https://example.com/docs".into(),
        },
        IrAction::Postback {
            title: "Ack".into(),
            data: json!({ "ok": true }),
        },
        IrAction::Postback {
            title: "More".into(),
            data: json!({ "more": true }),
        },
        IrAction::Postback {
            title: "Extra".into(),
            data: json!({ "extra": true }),
        },
    ];
    for action in base_actions.iter().take(action_count) {
        ir.actions.push(action.clone());
    }

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
