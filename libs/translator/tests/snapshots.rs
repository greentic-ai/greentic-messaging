use gsm_core::{
    make_tenant_ctx, CardAction, CardBlock, MessageCard, OutKind, OutMessage, Platform,
};
use gsm_translator::{TelegramTranslator, Translator, WebChatTranslator};

#[test]
fn telegram_text_snapshot() {
    let t = TelegramTranslator::new();
    let out = OutMessage {
        ctx: make_tenant_ctx("acme".into(), None, None),
        tenant: "acme".into(),
        platform: Platform::Telegram,
        chat_id: "c1".into(),
        thread_id: None,
        kind: OutKind::Text,
        text: Some("Hello <Greentic>".into()),
        message_card: None,
        meta: Default::default(),
    };
    let payloads = t.to_platform(&out).unwrap();
    insta::assert_json_snapshot!("tg_text", &payloads);
}

#[test]
fn telegram_card_snapshot() {
    let t = TelegramTranslator::new();
    let out = OutMessage {
        ctx: make_tenant_ctx("acme".into(), None, None),
        tenant: "acme".into(),
        platform: Platform::Telegram,
        chat_id: "c1".into(),
        thread_id: None,
        kind: OutKind::Card,
        text: None,
        message_card: Some(MessageCard {
            title: Some("Weather".into()),
            body: vec![
                CardBlock::Text {
                    text: "Day 1".into(),
                    markdown: true,
                },
                CardBlock::Fact {
                    label: "High".into(),
                    value: "20°C".into(),
                },
            ],
            actions: vec![
                CardAction::OpenUrl {
                    title: "Open".into(),
                    url: "https://app.greentic.ai".into(),
                    jwt: false,
                },
                CardAction::Postback {
                    title: "Refresh".into(),
                    data: serde_json::json!({"a":"b"}),
                },
            ],
        }),
        meta: Default::default(),
    };
    let payloads = t.to_platform(&out).unwrap();
    insta::assert_json_snapshot!("tg_card", &payloads);
}

#[test]
fn webchat_card_snapshot() {
    let t = WebChatTranslator::new();
    let out = OutMessage {
        ctx: make_tenant_ctx("acme".into(), None, None),
        tenant: "acme".into(),
        platform: Platform::WebChat,
        chat_id: "c1".into(),
        thread_id: None,
        kind: OutKind::Card,
        text: None,
        message_card: Some(MessageCard {
            title: Some("Weather".into()),
            body: vec![CardBlock::Text {
                text: "Hi".into(),
                markdown: true,
            }],
            actions: vec![],
        }),
        meta: Default::default(),
    };
    let payloads = t.to_platform(&out).unwrap();
    insta::assert_json_snapshot!("webchat_card", &payloads);
}
