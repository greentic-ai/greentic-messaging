#[cfg(feature = "adaptive-cards")]
mod adaptive_snapshots {
    use gsm_core::messaging_card::ir::{InputChoice, InputKind};
    use gsm_core::messaging_card::tier::Tier;
    use gsm_core::messaging_card::{
        MessageCardEngine, MessageCardIr, MessageCardIrBuilder, TeamsRenderer, WebChatRenderer,
    };
    use gsm_core::{AppLink, PlatformRenderer};
    use serde_json::{Value, json};

    #[test]
    fn teams_renders_ir_snapshot() {
        let renderer = TeamsRenderer;
        let mut ir = sample_ir();
        ir.meta.adaptive_payload = None;
        let rendered = renderer.render(&ir);
        assert_eq!(rendered.payload, load_fixture("teams/basic.json"));
    }

    #[test]
    fn webchat_renders_ir_snapshot() {
        let renderer = WebChatRenderer;
        let mut ir = sample_ir();
        ir.meta.adaptive_payload = None;
        let rendered = renderer.render(&ir);
        assert_eq!(rendered.payload, load_fixture("bf_webchat/basic.json"));
    }

    #[test]
    fn teams_prefers_adaptive_payload() {
        let renderer = TeamsRenderer;
        let mut ir = sample_ir();
        let adaptive = json!({
            "type": "AdaptiveCard",
            "$schema": "http://adaptivecards.io/schemas/adaptive-card.json",
            "version": "1.6",
            "body": [
                { "type": "TextBlock", "text": "Native", "wrap": true }
            ]
        });
        ir.meta.adaptive_payload = Some(adaptive.clone());
        let rendered = renderer.render(&ir);
        assert_eq!(rendered.payload, adaptive);
    }

    #[test]
    fn engine_render_invokes_registry_and_downgrade() {
        let engine = MessageCardEngine::bootstrap();
        let ir = sample_ir();
        let rendered = engine.render("teams", &ir).expect("renderer exists");
        assert_eq!(rendered["type"], "AdaptiveCard");
    }

    fn sample_ir() -> MessageCardIr {
        let builder = MessageCardIrBuilder::default()
            .tier(Tier::Premium)
            .title("Snapshot Title")
            .primary_text("Body text", true)
            .image(
                "https://example.com/banner.png".into(),
                Some("Banner".into()),
            )
            .fact("Status", "Green")
            .input(
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
            )
            .postback("Ack", json!({"ok": true}))
            .open_url("Docs", "https://example.com/docs");
        let mut ir = builder.build();
        ir.head.footer = Some("Footer".into());
        ir.meta.app_link = Some(AppLink {
            base_url: "https://premium.example/deeplink".into(),
            secret: Some("secret-token".into()),
            tenant: None,
            scope: None,
            state: None,
            jwt: None,
        });
        ir
    }

    fn load_fixture(path: &str) -> Value {
        let base = format!("tests/fixtures/renderers/{path}");
        let data = std::fs::read_to_string(base).expect("fixture missing");
        serde_json::from_str(&data).expect("invalid json")
    }
}

#[cfg(not(feature = "adaptive-cards"))]
#[test]
fn adaptive_snapshots_skipped() {
    // Feature disabled; nothing to verify.
}
