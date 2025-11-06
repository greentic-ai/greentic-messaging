use std::collections::BTreeMap;

use serde_json::{Map, Value};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    session::WebchatSession,
    types::{ConversationRef, IncomingMessage, MessagePayload, Participant},
};

pub fn normalize_activity(session: &WebchatSession, activity: &Value) -> Option<IncomingMessage> {
    let obj = activity.as_object()?;
    let activity_type = obj.get("type").and_then(Value::as_str).unwrap_or("event");

    let from = obj.get("from").and_then(Value::as_object)?;
    let from_id = from.get("id").and_then(Value::as_str)?.to_string();
    let from_name = from
        .get("name")
        .and_then(Value::as_str)
        .map(|s| s.to_string());
    let from_role = from
        .get("role")
        .and_then(Value::as_str)
        .map(|s| s.to_string());

    let id = obj
        .get("id")
        .and_then(Value::as_str)
        .map(|s| s.to_string())
        .unwrap_or_else(|| Uuid::new_v4().to_string());

    let timestamp = obj
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(|ts| {
            OffsetDateTime::parse(ts, &time::format_description::well_known::Rfc3339).ok()
        })
        .unwrap_or_else(OffsetDateTime::now_utc);

    let payload = match activity_type {
        "message" => message_payload(obj),
        "typing" => Some(MessagePayload::Typing),
        "event" => event_payload(obj),
        "invoke" => invoke_payload(obj),
        other => Some(MessagePayload::Event {
            name: format!("bf.{other}"),
            value: Some(Value::Object(obj.clone())),
        }),
    }?;

    let channel_data = obj
        .get("channelData")
        .and_then(Value::as_object)
        .map(|m| to_btreemap(m.clone()))
        .unwrap_or_default();

    Some(IncomingMessage {
        id,
        at: timestamp,
        tenant_ctx: session.tenant_ctx.clone(),
        conversation: ConversationRef {
            channel: "webchat".into(),
            conversation_id: session.conversation_id.clone(),
        },
        from: Participant {
            id: from_id,
            name: from_name,
            role: from_role,
        },
        payload,
        channel_data,
        raw_activity: activity.clone(),
    })
}

fn message_payload(obj: &Map<String, Value>) -> Option<MessagePayload> {
    if let Some(attachments) = obj.get("attachments").and_then(Value::as_array)
        && let Some(preferred) = attachments
            .iter()
            .find(|attachment| adaptive_card_content_type(attachment))
            .or_else(|| attachments.first())
    {
        let content_type = preferred
            .get("contentType")
            .or_else(|| preferred.get("content_type"))
            .and_then(Value::as_str)
            .map(|s| s.to_string())
            .unwrap_or_else(|| "application/octet-stream".into());
        let content = preferred.get("content").cloned().unwrap_or(Value::Null);
        return Some(MessagePayload::Attachment {
            content_type,
            content,
        });
    }

    let text = obj.get("text").and_then(Value::as_str)?.to_string();
    let locale = obj
        .get("locale")
        .and_then(Value::as_str)
        .map(|s| s.to_string());
    Some(MessagePayload::Text { text, locale })
}

fn event_payload(obj: &Map<String, Value>) -> Option<MessagePayload> {
    let name = obj.get("name").and_then(Value::as_str)?.to_string();
    let value = obj.get("value").cloned();
    Some(MessagePayload::Event { name, value })
}

fn invoke_payload(obj: &Map<String, Value>) -> Option<MessagePayload> {
    let name = obj.get("name").and_then(Value::as_str)?;
    if name.eq_ignore_ascii_case("adaptiveCard/action") {
        let value = obj.get("value").cloned();
        return Some(MessagePayload::Event {
            name: "adaptiveCard/action".to_string(),
            value,
        });
    }

    Some(MessagePayload::Event {
        name: name.to_string(),
        value: obj.get("value").cloned(),
    })
}

fn adaptive_card_content_type(attachment: &Value) -> bool {
    attachment
        .get("contentType")
        .or_else(|| attachment.get("content_type"))
        .and_then(Value::as_str)
        .map(|ct| {
            ct.to_ascii_lowercase()
                .starts_with("application/vnd.microsoft.card.adaptive")
        })
        .unwrap_or(false)
}

fn to_btreemap(map: Map<String, Value>) -> BTreeMap<String, Value> {
    map.into_iter().collect()
}
