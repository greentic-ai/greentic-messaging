use gsm_core::{
    make_tenant_ctx, CardAction, CardBlock, MessageCard, OutKind, OutMessage, Platform,
};
use gsm_translator::webex::{
    parse_attachment_action, parse_message, to_webex_payload, WebexInboundEvent,
};
use gsm_translator::{Translator, WebexTranslator};

#[test]
fn outbound_text_payload() {
    let out = OutMessage {
        ctx: make_tenant_ctx("acme".into(), None, None),
        tenant: "acme".into(),
        platform: Platform::Webex,
        chat_id: "room-1".into(),
        thread_id: None,
        kind: OutKind::Text,
        text: Some("Hello".into()),
        message_card: None,
        meta: Default::default(),
    };
    let payload = to_webex_payload(&out).expect("payload");
    assert_eq!(payload["roomId"], "room-1");
    assert_eq!(payload["markdown"], "Hello");
}

#[test]
fn outbound_card_payload() {
    let card = MessageCard {
        title: Some("Card".into()),
        body: vec![CardBlock::Text {
            text: "Body".into(),
            markdown: true,
        }],
        actions: vec![CardAction::OpenUrl {
            title: "Open".into(),
            url: "https://example.com".into(),
            jwt: false,
        }],
    };
    let out = OutMessage {
        ctx: make_tenant_ctx("acme".into(), None, None),
        tenant: "acme".into(),
        platform: Platform::Webex,
        chat_id: "room-1".into(),
        thread_id: None,
        kind: OutKind::Card,
        text: Some("Intro".into()),
        message_card: Some(card),
        meta: Default::default(),
    };
    let payload = to_webex_payload(&out).expect("payload");
    let attachments = payload["attachments"].as_array().expect("attachments");
    assert_eq!(attachments.len(), 1);
    assert_eq!(
        attachments[0]["contentType"],
        "application/vnd.microsoft.card.adaptive"
    );
}

#[test]
fn outbound_via_translator_struct() {
    let translator = WebexTranslator::new();
    let out = OutMessage {
        ctx: make_tenant_ctx("acme".into(), None, None),
        tenant: "acme".into(),
        platform: Platform::Webex,
        chat_id: "room-1".into(),
        thread_id: None,
        kind: OutKind::Text,
        text: Some("Hi".into()),
        message_card: None,
        meta: Default::default(),
    };
    let payloads = translator.to_platform(&out).expect("payloads");
    assert_eq!(payloads.len(), 1);
    assert_eq!(payloads[0]["markdown"], "Hi");
}

#[test]
fn inbound_parse_card() {
    let payload = serde_json::json!({
        "markdown": "Hello",
        "attachments": [
            {
                "contentType": "application/vnd.microsoft.card.adaptive",
                "content": {
                    "type": "AdaptiveCard",
                    "version": "1.4",
                    "body": [
                        {"type": "TextBlock", "text": "Card", "weight": "Bolder"},
                        {"type": "TextBlock", "text": "Body", "weight": "Default"},
                        {"type": "Image", "url": "https://example.com/img.png"}
                    ],
                    "actions": [
                        {"type": "Action.OpenUrl", "title": "Open", "url": "https://example.com"}
                    ]
                }
            }
        ]
    });

    let events = parse_message(&payload).expect("events");
    assert!(events
        .iter()
        .any(|e| matches!(e, WebexInboundEvent::Text(t) if t == "Hello")));
    let card = events
        .iter()
        .find_map(|e| match e {
            WebexInboundEvent::Card(card) => Some(card),
            _ => None,
        })
        .expect("card");
    assert_eq!(card.title.as_deref(), Some("Card"));
}

#[test]
fn inbound_parse_postback() {
    let payload = serde_json::json!({
        "inputs": {
            "action": "approve",
            "id": "123"
        }
    });
    let event = parse_attachment_action(&payload).expect("postback");
    match event {
        WebexInboundEvent::Postback { data } => {
            assert_eq!(data["action"], "approve");
        }
        _ => panic!("expected postback"),
    }
}
