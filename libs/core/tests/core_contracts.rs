use gsm_core::*;

#[test]
fn envelope_validates() {
    let env = MessageEnvelope {
        tenant: "acme".into(),
        platform: Platform::Telegram,
        chat_id: "room-1".into(),
        user_id: "u-9".into(),
        thread_id: None,
        msg_id: "msg-123".into(),
        text: Some("hi".into()),
        timestamp: "2025-10-14T09:00:00Z".into(),
        context: Default::default(),
    };
    assert!(validate_envelope(&env).is_ok());
}

#[test]
fn envelope_invalid_timestamp() {
    let mut env = MessageEnvelope {
        tenant: "acme".into(),
        platform: Platform::Slack,
        chat_id: "room-1".into(),
        user_id: "u-9".into(),
        thread_id: None,
        msg_id: "msg-123".into(),
        text: Some("hi".into()),
        timestamp: "bad-time".into(),
        context: Default::default(),
    };
    assert!(validate_envelope(&env).is_err());
    env.timestamp = "2025-10-14T10:00:00Z".into();
    assert!(validate_envelope(&env).is_ok());
}

#[test]
fn subjects_helpers_ok() {
    assert_eq!(
        in_subject("acme", "teams", "chat/42"),
        "greentic.msg.in.acme.teams.chat-42"
    );
    assert_eq!(
        out_subject("acme", "telegram", "a b"),
        "greentic.msg.out.acme.telegram.a-b"
    );
    assert!(dlq_subject("out", "t", "p").starts_with("greentic.msg.dlq.out."));
    assert!(subs_subject("events", "t", "p").starts_with("greentic.subs.events."));
}

#[test]
fn out_text_and_card_validate() {
    let mut out = OutMessage {
        tenant: "acme".into(),
        platform: Platform::Teams,
        chat_id: "chat-1".into(),
        thread_id: None,
        kind: OutKind::Text,
        text: Some("hello".into()),
        message_card: None,
        meta: Default::default(),
    };
    assert!(validate_out(&out).is_ok());

    out.kind = OutKind::Card;
    out.text = None;
    out.message_card = Some(MessageCard {
        title: Some("Title".into()),
        body: vec![CardBlock::Text {
            text: "Body".into(),
            markdown: true,
        }],
        actions: vec![],
    });
    assert!(validate_out(&out).is_ok());
}
