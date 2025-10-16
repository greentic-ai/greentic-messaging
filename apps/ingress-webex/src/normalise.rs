use anyhow::{anyhow, Context, Result};
use gsm_core::{MessageEnvelope, Platform};
use gsm_translator::webex::{parse_attachment_action, parse_message, WebexInboundEvent};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Deserialize)]
pub struct WebexWebhook {
    #[serde(default)]
    pub resource: Option<String>,
    #[serde(default)]
    pub event: Option<String>,
    pub data: Option<WebexData>,
}

#[derive(Debug, Deserialize)]
pub struct WebexData {
    #[serde(rename = "id")]
    pub message_id: String,
    #[serde(rename = "roomId")]
    pub room_id: String,
    #[serde(rename = "personId")]
    pub person_id: String,
    #[serde(rename = "parentId")]
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(rename = "created")]
    pub created_at: String,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub markdown: Option<String>,
    #[serde(default)]
    pub attachments: Option<Vec<Value>>,
    #[serde(flatten)]
    #[serde(default)]
    pub additional_data: BTreeMap<String, Value>,
}

pub fn normalise_webhook(tenant: &str, raw: &Value) -> Result<MessageEnvelope> {
    let payload: WebexWebhook =
        serde_json::from_value(raw.clone()).context("failed to decode webex webhook json")?;

    let data = payload
        .data
        .ok_or_else(|| anyhow!("missing webhook data payload"))?;

    let mut context = BTreeMap::new();
    if let Some(resource) = payload.resource.clone() {
        context.insert("resource".into(), Value::String(resource));
    }
    if let Some(event) = payload.event.clone() {
        context.insert("event".into(), Value::String(event));
    }
    if let Some(attachments) = data.attachments.clone() {
        context.insert("attachments".into(), Value::Array(attachments));
    }
    if let Some(markdown) = data.markdown.clone() {
        context.insert("markdown".into(), Value::String(markdown));
    }
    for (key, value) in data.additional_data.iter() {
        context.entry(key.clone()).or_insert(value.clone());
    }

    let mut envelope = MessageEnvelope {
        tenant: tenant.to_string(),
        platform: Platform::Webex,
        chat_id: data.room_id.clone(),
        user_id: data.person_id.clone(),
        thread_id: data.parent_id.clone(),
        msg_id: data.message_id.clone(),
        text: data.text.or(data.markdown.clone()),
        timestamp: data.created_at.clone(),
        context,
    };

    enrich_with_events(&mut envelope, raw)?;

    Ok(envelope)
}

fn enrich_with_events(envelope: &mut MessageEnvelope, raw: &Value) -> Result<()> {
    let resource = raw
        .get("resource")
        .and_then(|v| v.as_str())
        .unwrap_or_default();

    match resource {
        "messages" => {
            if let Some(data) = raw.get("data") {
                for event in parse_message(data)? {
                    match event {
                        WebexInboundEvent::Text(text) => {
                            if envelope.text.is_none() {
                                envelope.text = Some(text);
                            }
                        }
                        WebexInboundEvent::Card(card) => {
                            envelope
                                .context
                                .insert("card".into(), serde_json::to_value(card)?);
                        }
                        WebexInboundEvent::Postback { data } => {
                            envelope.context.insert("postback".into(), data);
                        }
                    }
                }
            }
        }
        "attachmentActions" => {
            if let Some(data) = raw.get("data") {
                if let WebexInboundEvent::Postback { data } = parse_attachment_action(data)? {
                    envelope.context.insert("postback".into(), data);
                }
            }
        }
        _ => {}
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalises_basic_payload() {
        let body = r#"{
            "resource": "messages",
            "event": "created",
            "data": {
                "id": "mid-123",
                "roomId": "room-1",
                "personId": "person-9",
                "created": "2024-01-01T00:00:00Z",
                "text": "hello world"
            }
        }"#;
        let raw: Value = serde_json::from_str(body).unwrap();
        let env = normalise_webhook("acme", &raw).expect("envelope");
        assert_eq!(env.platform, Platform::Webex);
        assert_eq!(env.tenant, "acme");
        assert_eq!(env.chat_id, "room-1");
        assert_eq!(env.user_id, "person-9");
        assert_eq!(env.msg_id, "mid-123");
        assert_eq!(env.text.as_deref(), Some("hello world"));
        assert_eq!(env.timestamp, "2024-01-01T00:00:00Z");
    }

    #[test]
    fn captures_card_attachment() {
        let json = r#"{
            "resource": "messages",
            "event": "created",
            "data": {
                "id": "mid-234",
                "roomId": "room-9",
                "personId": "person-7",
                "created": "2024-01-01T00:00:00Z",
                "attachments": [
                    {
                        "contentType": "application/vnd.microsoft.card.adaptive",
                        "content": {
                            "type": "AdaptiveCard",
                            "version": "1.4",
                            "body": [
                                {"type": "TextBlock", "text": "Card", "weight": "Bolder"},
                                {"type": "TextBlock", "text": "Body"}
                            ],
                            "actions": [
                                {"type": "Action.OpenUrl", "title": "Open", "url": "https://example.com"}
                            ]
                        }
                    }
                ]
            }
        }"#;
        let raw: Value = serde_json::from_str(json).unwrap();
        let env = normalise_webhook("acme", &raw).expect("envelope");
        assert_eq!(env.context["card"]["title"], "Card");
    }

    #[test]
    fn captures_postback_inputs() {
        let json = r#"{
            "resource": "attachmentActions",
            "event": "created",
            "data": {
                "id": "act-1",
                "roomId": "room-9",
                "personId": "person-7",
                "created": "2024-01-01T00:00:00Z",
                "inputs": { "action": "ack", "id": "xyz" }
            }
        }"#;
        let raw: Value = serde_json::from_str(json).unwrap();
        let env = normalise_webhook("acme", &raw).expect("envelope");
        assert_eq!(env.context["postback"]["action"], "ack");
    }
}
