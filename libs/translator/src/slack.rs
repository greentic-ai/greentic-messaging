//! Helpers for translating `OutMessage` instances into Slack block payloads.

use crate::telemetry::translate_with_span;
use anyhow::{anyhow, Context, Result};
use gsm_core::{CardAction, CardBlock, MessageCard, OutKind, OutMessage};
use serde_json::{json, Value};

const MAX_BLOCKS_PER_MESSAGE: usize = 45;
const MAX_ACTIONS_PER_BLOCK: usize = 5;

pub fn to_slack_payloads(out: &OutMessage) -> Result<Vec<Value>> {
    translate_with_span(out, "slack", || {
        let thread_ts = out.thread_id.as_deref();
        match out.kind {
            OutKind::Text => {
                let text = out.text.clone().unwrap_or_default();
                let blocks = vec![section_md(&text)];
                Ok(vec![payload_with_blocks(&text, blocks, thread_ts)])
            }
            OutKind::Card => {
                let card = out
                    .message_card
                    .as_ref()
                    .context("missing card for OutKind::Card")?;
                let blocks = card_to_blocks(card, out)?;
                let title = card.title.as_deref().unwrap_or_default();
                let mut payloads = Vec::new();
                for chunk in blocks.chunks(MAX_BLOCKS_PER_MESSAGE) {
                    payloads.push(payload_with_blocks(title, chunk.to_vec(), thread_ts));
                }
                Ok(payloads)
            }
        }
    })
}

fn section_md(text: &str) -> Value {
    json!({
      "type": "section",
      "text": { "type": "mrkdwn", "text": text }
    })
}

fn payload_with_blocks(text: &str, blocks: Vec<Value>, thread_ts: Option<&str>) -> Value {
    let mut payload = json!({
      "text": text,
      "blocks": blocks,
    });
    if let Some(ts) = thread_ts {
        payload
            .as_object_mut()
            .unwrap()
            .insert("thread_ts".into(), json!(ts));
    }
    payload
}

fn card_to_blocks(card: &MessageCard, out: &OutMessage) -> Result<Vec<Value>> {
    let mut blocks: Vec<Value> = Vec::new();
    if let Some(title) = &card.title {
        blocks.push(json!({
          "type": "header",
          "text": { "type": "plain_text", "text": title, "emoji": true }
        }));
    }

    let mut fact_lines: Vec<String> = Vec::new();

    for block in &card.body {
        match block {
            CardBlock::Text { text, .. } => {
                flush_facts(&mut fact_lines, &mut blocks);
                blocks.push(section_md(text));
            }
            CardBlock::Fact { label, value } => {
                fact_lines.push(format!("• *{}*: {}", label, value));
            }
            CardBlock::Image { url } => {
                flush_facts(&mut fact_lines, &mut blocks);
                blocks.push(json!({
                  "type": "image",
                  "image_url": url,
                  "alt_text": "image"
                }));
            }
        }
    }

    flush_facts(&mut fact_lines, &mut blocks);

    if !card.actions.is_empty() {
        let mut elements = Vec::new();
        for (idx, action) in card.actions.iter().enumerate() {
            match action {
                CardAction::OpenUrl { title, url, .. } => {
                    let href = crate::secure_action_url(out, title, url);
                    elements.push(json!({
                      "type": "button",
                      "text": { "type": "plain_text", "text": title, "emoji": true },
                      "url": href
                    }));
                }
                CardAction::Postback { title, data } => {
                    let value = serde_json::to_string(data).unwrap_or_else(|_| "{}".into());
                    elements.push(json!({
                      "type": "button",
                      "text": { "type": "plain_text", "text": title, "emoji": true },
                      "action_id": format!("postback_{}", idx),
                      "value": value
                    }));
                }
            }
        }

        for chunk in elements.chunks(MAX_ACTIONS_PER_BLOCK) {
            blocks.push(json!({
              "type": "actions",
              "elements": chunk
            }));
        }
    }

    if blocks.is_empty() {
        return Err(anyhow!("card produced no slack blocks"));
    }

    Ok(blocks)
}

fn flush_facts(fact_lines: &mut Vec<String>, blocks: &mut Vec<Value>) {
    if !fact_lines.is_empty() {
        let text = fact_lines.join("\n");
        blocks.push(section_md(&text));
        fact_lines.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gsm_core::{
        make_tenant_ctx, CardAction, CardBlock, MessageCard, OutKind, OutMessage, Platform,
    };

    fn base_message(kind: OutKind) -> OutMessage {
        OutMessage {
            ctx: make_tenant_ctx("acme".into(), None, None),
            tenant: "acme".into(),
            platform: Platform::Slack,
            chat_id: "C123".into(),
            thread_id: Some("1710000000.000100".into()),
            kind,
            text: None,
            message_card: None,
            meta: Default::default(),
        }
    }

    #[test]
    fn text_payload_contains_section_and_thread() {
        let mut out = base_message(OutKind::Text);
        out.text = Some("Hello *world*!".into());
        let payloads = to_slack_payloads(&out).unwrap();
        assert_eq!(payloads.len(), 1);
        let payload = &payloads[0];
        assert_eq!(payload["text"], "Hello *world*!");
        assert_eq!(payload["thread_ts"], "1710000000.000100");
        assert_eq!(payload["blocks"][0]["type"], "section");
        assert_eq!(payload["blocks"][0]["text"]["text"], "Hello *world*!");
    }

    #[test]
    fn card_payload_builds_blocks_and_actions() {
        let mut out = base_message(OutKind::Card);
        out.message_card = Some(MessageCard {
            title: Some("Status Update".into()),
            body: vec![
                CardBlock::Text {
                    text: "Line one".into(),
                    markdown: true,
                },
                CardBlock::Fact {
                    label: "Env".into(),
                    value: "Prod".into(),
                },
                CardBlock::Image {
                    url: "https://example.com/image.png".into(),
                },
            ],
            actions: vec![
                CardAction::OpenUrl {
                    title: "View".into(),
                    url: "https://example.com".into(),
                    jwt: false,
                },
                CardAction::Postback {
                    title: "Ack".into(),
                    data: serde_json::json!({"ok": true}),
                },
            ],
        });

        let payloads = to_slack_payloads(&out).unwrap();
        assert_eq!(payloads.len(), 1);
        let blocks = payloads[0]["blocks"].as_array().unwrap();
        assert_eq!(blocks[0]["type"], "header");
        assert_eq!(blocks[1]["type"], "section");
        assert_eq!(blocks[2]["type"], "section");
        assert_eq!(blocks[3]["type"], "image");
        assert_eq!(blocks[4]["type"], "actions");
        assert_eq!(blocks[4]["elements"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn card_payload_paginates_blocks() {
        let mut out = base_message(OutKind::Card);
        out.message_card = Some(MessageCard {
            title: Some("Large".into()),
            body: (0..95)
                .map(|i| CardBlock::Text {
                    text: format!("Line {i}"),
                    markdown: false,
                })
                .collect(),
            actions: vec![],
        });

        let payloads = to_slack_payloads(&out).unwrap();
        assert_eq!(payloads.len(), 3);
        assert!(payloads
            .iter()
            .all(|p| p["thread_ts"] == "1710000000.000100"));
        assert!(payloads
            .iter()
            .all(|p| p["blocks"].as_array().unwrap().len() <= MAX_BLOCKS_PER_MESSAGE));
    }

    #[test]
    fn actions_chunk_to_multiple_blocks() {
        let mut out = base_message(OutKind::Card);
        out.message_card = Some(MessageCard {
            title: None,
            body: vec![CardBlock::Text {
                text: "Actions".into(),
                markdown: false,
            }],
            actions: (0..7)
                .map(|i| CardAction::OpenUrl {
                    title: format!("Button {i}"),
                    url: format!("https://example.com/{i}"),
                    jwt: false,
                })
                .collect(),
        });

        let payloads = to_slack_payloads(&out).unwrap();
        let blocks = payloads[0]["blocks"].as_array().unwrap();
        assert_eq!(blocks.iter().filter(|b| b["type"] == "actions").count(), 2);
        assert_eq!(
            blocks
                .iter()
                .filter(|b| b["type"] == "actions")
                .flat_map(|b| b["elements"].as_array().unwrap().iter())
                .count(),
            7
        );
    }
}
