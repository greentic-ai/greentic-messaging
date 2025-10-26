use anyhow::Result;
use gsm_core::{CardAction as CoreAction, CardBlock as CoreBlock, MessageCard, MessageEnvelope};
use handlebars::Handlebars;
use serde_json::json;

pub fn render_card(
    card: &crate::model::CardNode,
    hbs: &Handlebars<'static>,
    env: &MessageEnvelope,
    state: &serde_json::Value,
    payload: &serde_json::Value,
) -> Result<MessageCard> {
    // Render every string field via Handlebars
    let mut title = None;
    if let Some(t) = &card.title {
        title = Some(hbs.render_template(
            t,
            &json!({"envelope":env, "state":state, "payload":payload}),
        )?);
    }
    let mut body = vec![];
    for b in &card.body {
        match b {
            crate::model::CardBlock::Text { text, markdown } => {
                body.push(CoreBlock::Text {
                    text: hbs.render_template(
                        text,
                        &json!({"envelope":env, "state":state, "payload":payload}),
                    )?,
                    markdown: markdown.unwrap_or(true),
                });
            }
            crate::model::CardBlock::Fact { label, value } => {
                body.push(CoreBlock::Fact {
                    label: hbs.render_template(
                        label,
                        &json!({"envelope":env, "state":state, "payload":payload}),
                    )?,
                    value: hbs.render_template(
                        value,
                        &json!({"envelope":env, "state":state, "payload":payload}),
                    )?,
                });
            }
            crate::model::CardBlock::Image { url } => {
                body.push(CoreBlock::Image {
                    url: hbs.render_template(
                        url,
                        &json!({"envelope":env, "state":state, "payload":payload}),
                    )?,
                });
            }
        }
    }
    let mut actions = vec![];
    for a in &card.actions {
        match a {
            crate::model::CardAction::OpenUrl { title, url, jwt } => {
                actions.push(CoreAction::OpenUrl {
                    title: hbs.render_template(
                        title,
                        &json!({"envelope":env, "state":state, "payload":payload}),
                    )?,
                    url: hbs.render_template(
                        url,
                        &json!({"envelope":env, "state":state, "payload":payload}),
                    )?,
                    jwt: jwt.unwrap_or(false),
                })
            }
            crate::model::CardAction::Postback { title, data } => {
                let title = hbs.render_template(
                    title,
                    &json!({"envelope":env, "state":state, "payload":payload}),
                )?;
                let data_json = json!(data);
                actions.push(CoreAction::Postback {
                    title,
                    data: data_json,
                });
            }
        }
    }
    Ok(MessageCard {
        title,
        body,
        actions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CardAction, CardBlock, CardNode};
    use gsm_core::{MessageEnvelope, Platform};

    fn handlebars() -> &'static Handlebars<'static> {
        let mut hbs = Handlebars::new();
        hbs.register_escape_fn(handlebars::no_escape);
        Box::leak(Box::new(hbs))
    }

    fn sample_envelope() -> MessageEnvelope {
        MessageEnvelope {
            tenant: "acme".into(),
            platform: Platform::Slack,
            chat_id: "C123".into(),
            user_id: "user".into(),
            thread_id: None,
            msg_id: "msg-1".into(),
            text: Some("Hello".into()),
            timestamp: "2024-01-01T00:00:00Z".into(),
            context: Default::default(),
        }
    }

    #[test]
    fn render_card_applies_templates() {
        let card = CardNode {
            title: Some("Ticket for {{envelope.chat_id}}".into()),
            body: vec![
                CardBlock::Text {
                    text: "Score: {{state.score}}".into(),
                    markdown: Some(true),
                },
                CardBlock::Fact {
                    label: "Link".into(),
                    value: "{{payload.url}}".into(),
                },
            ],
            actions: vec![
                CardAction::OpenUrl {
                    title: "Open".into(),
                    url: "{{payload.url}}".into(),
                    jwt: Some(false),
                },
                CardAction::Postback {
                    title: "Ack".into(),
                    data: serde_json::json!({"done": true}),
                },
            ],
        };

        let env = sample_envelope();
        let state = serde_json::json!({"score": 42});
        let payload = serde_json::json!({"url": "https://example.com"});
        let rendered = render_card(&card, handlebars(), &env, &state, &payload).unwrap();

        assert_eq!(rendered.title.as_deref(), Some("Ticket for C123"));
        assert_eq!(rendered.body.len(), 2);
        assert_eq!(rendered.actions.len(), 2);

        match &rendered.body[0] {
            CoreBlock::Text { text, .. } => assert_eq!(text, "Score: 42"),
            _ => panic!("expected text block"),
        }
        match &rendered.body[1] {
            CoreBlock::Fact { value, .. } => assert_eq!(value, "https://example.com"),
            _ => panic!("expected fact block"),
        }
        match &rendered.actions[0] {
            CoreAction::OpenUrl { url, .. } => assert_eq!(url, "https://example.com"),
            _ => panic!("expected open url action"),
        }
    }
}
