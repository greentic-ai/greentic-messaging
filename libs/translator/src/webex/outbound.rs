use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use crate::{secure_action_url, translate_with_span};
use gsm_core::{CardAction, CardBlock, MessageCard, OutKind, OutMessage};

/// Build a Webex message payload from an internal [`OutMessage`].
pub fn to_webex_payload(out: &OutMessage) -> Result<Value> {
    translate_with_span(out, "webex", || build_payload(out))
}

fn build_payload(out: &OutMessage) -> Result<Value> {
    let mut map = serde_json::Map::new();
    map.insert("roomId".into(), Value::String(out.chat_id.clone()));

    match out.kind {
        OutKind::Text => {
            let text = out
                .text
                .clone()
                .ok_or_else(|| anyhow!("text payload missing for text message"))?;
            map.insert("markdown".into(), Value::String(text));
        }
        OutKind::Card => {
            let card = out
                .message_card
                .clone()
                .ok_or_else(|| anyhow!("missing message card for card payload"))?;
            let markdown = out.text.clone().unwrap_or_else(|| "".to_string());
            if !markdown.is_empty() {
                map.insert("markdown".into(), Value::String(markdown));
            }
            let attachment = json!({
                "contentType": "application/vnd.microsoft.card.adaptive",
                "content": card_to_adaptive(out, card)?,
            });
            map.insert("attachments".into(), Value::Array(vec![attachment]));
        }
    }

    Ok(Value::Object(map))
}

fn card_to_adaptive(out: &OutMessage, card: MessageCard) -> Result<Value> {
    let mut body: Vec<Value> = Vec::new();

    if let Some(title) = card.title {
        body.push(json!({
            "type": "TextBlock",
            "text": title,
            "weight": "Bolder",
            "size": "Medium",
            "wrap": true,
        }));
    }

    for block in card.body {
        match block {
            CardBlock::Text { text, markdown } => {
                body.push(json!({
                    "type": "TextBlock",
                    "text": text,
                    "wrap": true,
                    "isSubtle": false,
                    "weight": if markdown { "Bolder" } else { "Default" },
                }));
            }
            CardBlock::Fact { label, value } => {
                body.push(json!({
                    "type": "FactSet",
                    "facts": [{"title": label, "value": value}],
                }));
            }
            CardBlock::Image { url } => {
                body.push(json!({
                    "type": "Image",
                    "url": url,
                }));
            }
        }
    }

    let mut actions: Vec<Value> = Vec::new();
    for action in card.actions {
        match action {
            CardAction::OpenUrl { title, url, jwt } => {
                let href = secure_action_url(out, &title, &url);
                actions.push(json!({
                    "type": "Action.OpenUrl",
                    "title": title,
                    "url": href,
                    "requiresAuthentication": jwt,
                }));
            }
            CardAction::Postback { title, data } => {
                actions.push(json!({
                    "type": "Action.Submit",
                    "title": title,
                    "data": data,
                }));
            }
        }
    }

    Ok(json!({
        "type": "AdaptiveCard",
        "version": "1.4",
        "body": body,
        "actions": actions,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gsm_core::{make_tenant_ctx, CardBlock, MessageCard, Platform};

    fn sample_out(kind: OutKind, card: Option<MessageCard>) -> OutMessage {
        OutMessage {
            ctx: make_tenant_ctx("acme".into(), None, None),
            tenant: "acme".into(),
            platform: Platform::Webex,
            chat_id: "room-1".into(),
            thread_id: None,
            kind,
            text: Some("Hello".into()),
            message_card: card,
            meta: Default::default(),
        }
    }

    #[test]
    fn text_payload() {
        let out = sample_out(OutKind::Text, None);
        let payload = to_webex_payload(&out).expect("payload");
        assert_eq!(payload["roomId"], "room-1");
        assert_eq!(payload["markdown"], "Hello");
        assert!(payload.get("attachments").is_none());
    }

    #[test]
    fn card_payload() {
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
        let out = sample_out(OutKind::Card, Some(card));
        let payload = to_webex_payload(&out).expect("payload");
        let attachments = payload["attachments"].as_array().expect("attachments");
        assert_eq!(attachments.len(), 1);
        assert_eq!(
            attachments[0]["contentType"],
            "application/vnd.microsoft.card.adaptive"
        );
    }
}
