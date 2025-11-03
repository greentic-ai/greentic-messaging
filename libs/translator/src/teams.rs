//! Helpers for rendering Teams-specific Adaptive Cards.

use crate::telemetry::translate_with_span;
use anyhow::Result;
use gsm_core::{CardAction, CardBlock, MessageCard, OutMessage};
use serde_json::{Value, json};

/// Converts a [`MessageCard`](gsm_core::MessageCard) into a Teams Adaptive Card payload.
///
/// ```
/// use gsm_translator::teams::to_teams_adaptive;
/// use gsm_core::{CardAction, CardBlock, MessageCard};
///
/// let card = MessageCard {
///     title: Some("Weather".into()),
///     body: vec![
///         CardBlock::Text { text: "Sunny".into(), markdown: false },
///         CardBlock::Fact { label: "High".into(), value: "22C".into() },
///     ],
///     actions: vec![
///         CardAction::OpenUrl {
///             title: "Details".into(),
///             url: "https://example.com".into(),
///             jwt: false,
///         }
///     ],
/// };
///
/// let out = gsm_core::OutMessage {
///     ctx: gsm_core::make_tenant_ctx("acme".into(), None, None),
///     tenant: "acme".into(),
///     platform: gsm_core::Platform::Teams,
///     chat_id: "chat-1".into(),
///     thread_id: None,
///     kind: gsm_core::OutKind::Card,
///     text: None,
///     message_card: None,
///     meta: Default::default(),
/// };
/// let card_payload = to_teams_adaptive(&card, &out).unwrap();
/// assert_eq!(card_payload["type"], "AdaptiveCard");
/// assert_eq!(card_payload["body"][0]["text"], "Weather");
/// ```
pub fn to_teams_adaptive(card: &MessageCard, out: &OutMessage) -> Result<Value> {
    translate_with_span(out, "teams", || {
        let mut body: Vec<Value> = vec![];
        if let Some(t) = &card.title {
            body.push(json!({"type":"TextBlock","weight":"Bolder","size":"Medium","text":t}));
        }
        let mut facts: Vec<Value> = vec![];

        for b in &card.body {
            match b {
                CardBlock::Text { text, .. } => {
                    body.push(json!({
                      "type":"TextBlock",
                      "wrap": true,
                      "text": text
                    }));
                }
                CardBlock::Fact { label, value } => {
                    facts.push(json!({"title": label, "value": value}));
                }
                CardBlock::Image { url } => {
                    body.push(json!({"type":"Image","url":url}));
                }
            }
        }
        if !facts.is_empty() {
            body.push(json!({"type":"FactSet","facts": facts}));
        }

        let mut actions: Vec<Value> = vec![];
        for a in &card.actions {
            match a {
                CardAction::OpenUrl { title, url, .. } => {
                    let href = crate::secure_action_url(out, title, url);
                    actions.push(json!({
                      "type":"Action.OpenUrl",
                      "title": title,
                      "url": href
                    }));
                }
                CardAction::Postback { title, data } => {
                    actions.push(json!({
                      "type":"Action.Submit",
                      "title": title,
                      "data": data
                    }));
                }
            }
        }

        Ok(json!({
          "type":"AdaptiveCard",
          "version":"1.4",
          "body": body,
          "actions": actions,
          "$schema": "http://adaptivecards.io/schemas/adaptive-card.json"
        }))
    })
}
