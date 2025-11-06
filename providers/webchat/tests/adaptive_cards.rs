use greentic_messaging_providers_webchat::activity_bridge::normalize_activity;
use greentic_messaging_providers_webchat::session::WebchatSession;
use greentic_messaging_providers_webchat::types::MessagePayload;
use greentic_types::{EnvId, TenantCtx, TenantId};
use serde_json::json;

fn tenant_ctx() -> TenantCtx {
    TenantCtx::new(EnvId::from("dev"), TenantId::from("acme"))
}

#[test]
fn adaptive_card_attachment_pass_through() {
    let session = WebchatSession::new("conv-ac-1".to_string(), tenant_ctx(), "token".into());
    let activity = json!({
        "type": "message",
        "id": "msg-ac-1",
        "timestamp": "2024-01-01T00:00:00Z",
        "attachments": [
            {
                "contentType": "application/vnd.microsoft.card.adaptive",
                "content": {
                    "type": "AdaptiveCard",
                    "version": "1.5",
                    "body": [
                        {"type": "TextBlock", "text": "Adaptive hello"}
                    ]
                }
            }
        ],
        "from": {"id": "bot"}
    });

    let message = normalize_activity(&session, &activity).expect("expected adaptive card");
    match message.payload {
        MessagePayload::Attachment {
            ref content_type,
            ref content,
        } => {
            assert_eq!(content_type, "application/vnd.microsoft.card.adaptive");
            assert_eq!(content["type"], "AdaptiveCard");
            assert_eq!(content["body"][0]["text"], "Adaptive hello");
        }
        other => panic!("unexpected payload: {other:?}"),
    }
}

#[test]
fn adaptive_card_invoke_submit_normalizes_to_event() {
    let session = WebchatSession::new("conv-ac-2".to_string(), tenant_ctx(), "token".into());
    let submit_payload = json!({
        "action": {
            "type": "Action.Submit",
            "data": { "ticketId": "42", "comment": "On it" }
        }
    });
    let activity = json!({
        "type": "invoke",
        "name": "adaptiveCard/action",
        "id": "invoke-1",
        "timestamp": "2024-01-01T00:00:05Z",
        "value": submit_payload,
        "from": {"id": "user-123", "name": "Sam"}
    });

    let message = normalize_activity(&session, &activity).expect("expected invoke");
    match message.payload {
        MessagePayload::Event {
            ref name,
            ref value,
        } => {
            assert_eq!(name, "adaptiveCard/action");
            assert_eq!(value.as_ref().unwrap()["action"]["type"], "Action.Submit");
            assert_eq!(value.as_ref().unwrap()["action"]["data"]["ticketId"], "42");
        }
        other => panic!("unexpected payload: {other:?}"),
    }
}
