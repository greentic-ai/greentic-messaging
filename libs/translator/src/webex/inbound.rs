use anyhow::{anyhow, Context, Result};
use serde_json::Value;

use gsm_core::{CardAction, CardBlock, MessageCard};

/// Minimal inbound events extracted from Webex payloads.
#[derive(Debug, Clone, PartialEq)]
pub enum WebexInboundEvent {
    Text(String),
    Card(MessageCard),
    Postback { data: Value },
}

/// Parse a Webex message payload (as delivered by `resource=messages`).
pub fn parse_message(value: &Value) -> Result<Vec<WebexInboundEvent>> {
    let mut events = Vec::new();

    if let Some(text) = value
        .get("markdown")
        .and_then(|v| v.as_str())
        .or_else(|| value.get("text").and_then(|v| v.as_str()))
    {
        if !text.trim().is_empty() {
            events.push(WebexInboundEvent::Text(text.to_string()));
        }
    }

    if let Some(attachments) = value.get("attachments").and_then(|v| v.as_array()) {
        for attachment in attachments {
            let content_type = attachment
                .get("contentType")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            if content_type.eq_ignore_ascii_case("application/vnd.microsoft.card.adaptive") {
                if let Some(content) = attachment.get("content") {
                    if let Ok(card) = adaptive_to_card(content) {
                        events.push(WebexInboundEvent::Card(card));
                    }
                }
            }
        }
    }

    Ok(events)
}

/// Parse a Webex attachment action payload (`resource=attachmentActions`).
pub fn parse_attachment_action(value: &Value) -> Result<WebexInboundEvent> {
    let data = value
        .get("inputs")
        .cloned()
        .or_else(|| value.get("data").cloned())
        .unwrap_or_else(|| Value::Object(Default::default()));
    Ok(WebexInboundEvent::Postback { data })
}

fn adaptive_to_card(value: &Value) -> Result<MessageCard> {
    let body = value
        .get("body")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow!("adaptive card missing body"))?;

    let mut title: Option<String> = None;
    let mut blocks: Vec<CardBlock> = Vec::new();

    for element in body {
        let typ = element
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        match typ {
            "TextBlock" => {
                let text = element
                    .get("text")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow!("text block missing text"))?;
                let weight = element
                    .get("weight")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                if title.is_none() && weight.eq_ignore_ascii_case("bolder") {
                    title = Some(text.to_string());
                } else {
                    let markdown = element
                        .get("weight")
                        .and_then(|v| v.as_str())
                        .map(|w| w.eq_ignore_ascii_case("bolder"))
                        .unwrap_or(false);
                    blocks.push(CardBlock::Text {
                        text: text.to_string(),
                        markdown,
                    });
                }
            }
            "Image" => {
                if let Some(url) = element.get("url").and_then(|v| v.as_str()) {
                    blocks.push(CardBlock::Image {
                        url: url.to_string(),
                    });
                }
            }
            "FactSet" => {
                if let Some(facts) = element.get("facts").and_then(|v| v.as_array()) {
                    for fact in facts {
                        if let (Some(label), Some(value)) = (
                            fact.get("title").and_then(|v| v.as_str()),
                            fact.get("value").and_then(|v| v.as_str()),
                        ) {
                            blocks.push(CardBlock::Fact {
                                label: label.to_string(),
                                value: value.to_string(),
                            });
                        }
                    }
                }
            }
            _ => {}
        }
    }

    let mut actions: Vec<CardAction> = Vec::new();
    if let Some(items) = value.get("actions").and_then(|v| v.as_array()) {
        for item in items {
            let typ = item
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            match typ {
                "Action.OpenUrl" => {
                    let title = item
                        .get("title")
                        .and_then(|v| v.as_str())
                        .context("missing open url title")?;
                    let url = item
                        .get("url")
                        .and_then(|v| v.as_str())
                        .context("missing open url")?;
                    let requires_auth = item
                        .get("requiresAuthentication")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    actions.push(CardAction::OpenUrl {
                        title: title.to_string(),
                        url: url.to_string(),
                        jwt: requires_auth,
                    });
                }
                "Action.Submit" => {
                    let title = item
                        .get("title")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Submit");
                    let data = item
                        .get("data")
                        .cloned()
                        .unwrap_or_else(|| Value::Object(Default::default()));
                    actions.push(CardAction::Postback {
                        title: title.to_string(),
                        data,
                    });
                }
                _ => {}
            }
        }
    }

    Ok(MessageCard {
        title,
        body: blocks,
        actions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_text_and_card() {
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
        assert!(events
            .iter()
            .any(|e| matches!(e, WebexInboundEvent::Card(_))));
    }

    #[test]
    fn parses_postback() {
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
}
