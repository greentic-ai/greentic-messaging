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
