use gsm_core::{
    CardAction, CardBlock, ChannelMessage, MessageCard, MessageEnvelope, OutKind, OutMessage,
    OutboundEnvelope, Platform,
};
use serde_json::json;

#[test]
fn channel_message_roundtrips_via_json() {
    let message = ChannelMessage {
        tenant: gsm_core::make_tenant_ctx(
            "acme".into(),
            Some("team-1".into()),
            Some("user-9".into()),
        ),
        channel_id: "webchat".into(),
        session_id: "sess-123".into(),
        route: Some("default".into()),
        payload: json!({"msg_id": "m-1", "text": "hello"}),
    };

    assert_roundtrip(&message);
}

#[test]
fn message_envelope_roundtrips() {
    let mut env = MessageEnvelope {
        tenant: "acme".into(),
        platform: Platform::Slack,
        chat_id: "room-42".into(),
        user_id: "user-7".into(),
        thread_id: Some("thread-9".into()),
        msg_id: "msg-9".into(),
        text: Some("hi".into()),
        timestamp: "2024-01-01T00:00:00Z".into(),
        context: Default::default(),
    };
    env.context.insert("locale".into(), json!("en-US"));

    assert_roundtrip(&env);
}

#[test]
fn out_message_roundtrips_with_card() {
    let ctx = gsm_core::make_tenant_ctx("acme".into(), None, Some("user-1".into()));
    let card = MessageCard {
        title: Some("Weather".into()),
        body: vec![CardBlock::Text {
            text: "Sunny".into(),
            markdown: false,
        }],
        actions: vec![CardAction::Postback {
            title: "Ack".into(),
            data: json!({"ok": true}),
        }],
    };
    let out = OutMessage {
        ctx,
        tenant: "acme".into(),
        platform: Platform::Teams,
        chat_id: "chat-1".into(),
        thread_id: Some("thread-2".into()),
        kind: OutKind::Card,
        text: None,
        message_card: Some(card),
        #[cfg(feature = "adaptive-cards")]
        adaptive_card: None,
        meta: Default::default(),
    };

    assert_roundtrip(&out);
}

#[test]
fn outbound_envelope_preserves_session_and_channel() {
    let channel = ChannelMessage {
        tenant: gsm_core::make_tenant_ctx("acme".into(), None, Some("user-1".into())),
        channel_id: "webchat".into(),
        session_id: "sess-55".into(),
        route: None,
        payload: json!({"text": "hello"}),
    };
    let outbound = OutboundEnvelope::for_channel(&channel, json!({"reply": "hi"}));
    assert_eq!(outbound.session_id, "sess-55");
    assert_eq!(outbound.channel_id, "webchat");
}

#[test]
fn channel_message_requires_session_id() {
    let message = ChannelMessage {
        tenant: gsm_core::make_tenant_ctx("acme".into(), None, Some("user-1".into())),
        channel_id: "webchat".into(),
        session_id: "sess-1".into(),
        route: None,
        payload: json!({"msg_id": "1"}),
    };
    let mut value = serde_json::to_value(message).expect("serialize");
    value.as_object_mut().expect("object").remove("session_id");

    let result = serde_json::from_value::<ChannelMessage>(value);
    assert!(
        result.is_err(),
        "missing session_id should fail to deserialize"
    );
}

#[tokio::test]
async fn correlation_falls_back_to_message_id() {
    use gsm_core::{
        InMemoryWorkerClient, WorkerMessage, WorkerRoutingConfig, empty_worker_response_for,
        forward_to_worker,
    };

    let channel = ChannelMessage {
        tenant: gsm_core::make_tenant_ctx("acme".into(), None, Some("user-9".into())),
        channel_id: "webchat".into(),
        session_id: "sess-999".into(),
        route: None,
        payload: json!({"msg_id": "msg-abc"}),
    };
    let config = WorkerRoutingConfig::default();
    let payload = json!({"kind": "noop"});
    let client = InMemoryWorkerClient::new(|req| {
        assert_eq!(req.correlation_id.as_deref(), Some("msg-abc"));
        let mut resp = empty_worker_response_for(&req);
        resp.messages = vec![WorkerMessage {
            kind: "text".into(),
            payload_json: serde_json::to_string(&json!({"ok": true})).unwrap(),
        }];
        resp
    });

    let outbound = forward_to_worker(&client, &channel, payload, &config, None)
        .await
        .expect("forward worker");
    assert_eq!(outbound.len(), 1);
    assert_eq!(outbound[0].meta["correlation_id"], "msg-abc");
}

#[test]
fn webchat_error_classification_is_stable() {
    use axum::response::IntoResponse;
    use gsm_core::platforms::webchat::{DirectLineError, WebChatError};
    use reqwest::StatusCode;
    use std::time::Duration;

    let err = WebChatError::DirectLine(DirectLineError::Remote {
        status: StatusCode::TOO_MANY_REQUESTS,
        retry_after: Some(Duration::from_secs(5)),
        message: "rate limited".into(),
    });
    let status = err.status();
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let response = err.into_response();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let retry_after = response
        .headers()
        .get(axum::http::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    assert_eq!(retry_after.as_deref(), Some("5"));
}

fn assert_roundtrip<T>(value: &T)
where
    T: serde::Serialize + for<'de> serde::Deserialize<'de>,
{
    let json = serde_json::to_value(value).expect("serialize");
    let restored: T = serde_json::from_value(json.clone()).expect("deserialize");
    let json_restored = serde_json::to_value(restored).expect("serialize restored");
    assert_eq!(json, json_restored);
}
