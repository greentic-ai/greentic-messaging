use serde_json::{Value, json};
use tracing::warn;

use crate::messaging_card::ir::{Element, IrAction, MessageCardIr};
use crate::messaging_card::tier::Tier;

use super::{PlatformRenderer, RenderOutput, resolve_open_url};

const FACTSET_WARNING: &str = "webex.factset_downgraded";
const INPUT_WARNING: &str = "webex.inputs_not_supported";

#[derive(Default)]
pub struct WebexRenderer;

impl PlatformRenderer for WebexRenderer {
    fn platform(&self) -> &'static str {
        "webex"
    }

    fn target_tier(&self) -> Tier {
        Tier::Advanced
    }

    fn render(&self, ir: &MessageCardIr) -> RenderOutput {
        let mut warnings = Vec::new();
        let mut body = Vec::new();

        if let Some(title) = &ir.head.title {
            body.push(json!({
                "type": "TextBlock",
                "text": title,
                "wrap": true,
                "weight": "Bolder",
                "size": "Medium",
            }));
        }

        if let Some(subtitle) = &ir.head.text {
            body.push(json!({
                "type": "TextBlock",
                "text": subtitle,
                "wrap": true,
                "isSubtle": true,
            }));
        }

        for element in &ir.elements {
            match element {
                Element::Text { text, .. } => body.push(text_block(text)),
                Element::Image { url, alt } => {
                    body.push(json!({
                        "type": "Image",
                        "url": url,
                        "altText": alt.as_deref().unwrap_or("image"),
                    }));
                }
                Element::FactSet { facts } => {
                    if facts.is_empty() {
                        continue;
                    }
                    let lines: Vec<String> = facts
                        .iter()
                        .map(|fact| format!("*{}*: {}", fact.label, fact.value))
                        .collect();
                    body.push(text_block(&lines.join("\n")));
                    warnings.push(FACTSET_WARNING.into());
                    warn!(
                        target = "gsm.mcard.webex",
                        "downgrading fact set to text block"
                    );
                }
                Element::Input { .. } => {
                    warnings.push(INPUT_WARNING.into());
                    warn!(target = "gsm.mcard.webex", "webex does not support inputs");
                }
            }
        }

        if let Some(footer) = &ir.head.footer {
            body.push(json!({
                "type": "TextBlock",
                "text": footer,
                "wrap": true,
                "spacing": "Small",
                "isSubtle": true,
                "size": "Small",
            }));
        }

        let actions: Vec<Value> = ir
            .actions
            .iter()
            .map(|action| match action {
                IrAction::OpenUrl { title, url } => json!({
                    "type": "Action.OpenUrl",
                    "title": title,
                    "url": resolve_open_url(&ir.meta, url),
                }),
                IrAction::Postback { title, data } => json!({
                    "type": "Action.Submit",
                    "title": title,
                    "data": data,
                }),
            })
            .collect();

        let payload = json!({
            "type": "AdaptiveCard",
            "version": "1.4",
            "body": body,
            "actions": actions,
        });

        let mut output = RenderOutput::new(payload);
        output.warnings = warnings;
        output
    }
}

fn text_block(text: &str) -> Value {
    json!({
        "type": "TextBlock",
        "text": text,
        "wrap": true,
    })
}
