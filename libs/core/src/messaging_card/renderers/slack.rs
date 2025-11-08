use serde_json::{Value, json};

use crate::messaging_card::ir::{Element, InputChoice, InputKind, IrAction, MessageCardIr};
use crate::messaging_card::tier::Tier;

use super::{PlatformRenderer, RenderOutput, resolve_open_url};

const HEADER_LIMIT: usize = 150;
const MODAL_TITLE_LIMIT: usize = 24;
const BUTTON_LIMIT: usize = 5;

#[derive(Default)]
pub struct SlackRenderer;

impl PlatformRenderer for SlackRenderer {
    fn platform(&self) -> &'static str {
        "slack"
    }

    fn target_tier(&self) -> Tier {
        Tier::Advanced
    }

    fn render(&self, ir: &MessageCardIr) -> RenderOutput {
        let mut warnings = Vec::new();
        let has_inputs = ir
            .elements
            .iter()
            .any(|el| matches!(el, Element::Input { .. }));

        let payload = if has_inputs {
            render_modal(ir, &mut warnings)
        } else {
            json!({ "blocks": render_blocks(ir, &mut warnings, false) })
        };

        RenderOutput {
            payload,
            used_modal: has_inputs,
            warnings,
        }
    }
}

fn render_modal(ir: &MessageCardIr, warnings: &mut Vec<String>) -> Value {
    let title = truncate(
        ir.head
            .title
            .as_deref()
            .or_else(|| ir.head.text.as_deref())
            .unwrap_or("Card"),
        MODAL_TITLE_LIMIT,
    );

    json!({
        "type": "modal",
        "title": plain_text(&title),
        "submit": plain_text("Submit"),
        "close": plain_text("Close"),
        "callback_id": "gsm_card_modal",
        "blocks": render_blocks(ir, warnings, true),
    })
}

fn render_blocks(
    ir: &MessageCardIr,
    warnings: &mut Vec<String>,
    include_inputs: bool,
) -> Vec<Value> {
    let mut blocks = Vec::new();

    if !include_inputs {
        if let Some(title) = &ir.head.title {
            blocks.push(json!({
                "type": "header",
                "text": plain_text(&truncate(title, HEADER_LIMIT)),
            }));
        }
    }

    let mut saw_text_element = false;

    for element in &ir.elements {
        match element {
            Element::Text { text, markdown } => {
                saw_text_element = true;
                blocks.push(section_block(text, *markdown));
            }
            Element::Image { url, alt } => {
                blocks.push(json!({
                    "type": "image",
                    "image_url": url,
                    "alt_text": alt.as_deref().unwrap_or("image"),
                }));
            }
            Element::FactSet { facts } => {
                if facts.is_empty() {
                    continue;
                }
                let mut fields = Vec::new();
                for fact in facts {
                    if fields.len() == 10 {
                        warnings.push("slack.factset_truncated".into());
                        break;
                    }
                    fields.push(json!({
                        "type": "mrkdwn",
                        "text": format!("*{}*\n{}", fact.label, fact.value)
                    }));
                }
                if !fields.is_empty() {
                    blocks.push(json!({
                        "type": "section",
                        "fields": fields,
                    }));
                }
            }
            Element::Input {
                label,
                kind,
                id,
                required,
                choices,
            } => {
                if include_inputs {
                    if let Some(block) = input_block(
                        label.as_deref(),
                        kind,
                        id.as_deref(),
                        *required,
                        choices,
                        warnings,
                    ) {
                        blocks.push(block);
                    }
                } else {
                    warnings.push("slack.inputs_require_modal".into());
                }
            }
        }
    }

    if !saw_text_element {
        if let Some(text) = &ir.head.text {
            blocks.push(section_block(text, true));
        }
    }

    if let Some(footer) = &ir.head.footer {
        blocks.push(json!({
            "type": "context",
            "elements": [
                json!({
                    "type": "mrkdwn",
                    "text": footer,
                })
            ],
        }));
    }

    if let Some(actions) = actions_block(ir, warnings) {
        blocks.push(actions);
    }

    blocks
}

fn input_block(
    label: Option<&str>,
    kind: &InputKind,
    id: Option<&str>,
    required: bool,
    choices: &[InputChoice],
    warnings: &mut Vec<String>,
) -> Option<Value> {
    let block_id = id.unwrap_or("input").to_string();
    match kind {
        InputKind::Text => Some(json!({
            "type": "input",
            "block_id": block_id,
            "label": plain_text(label.unwrap_or("Input")),
            "optional": !required,
            "element": {
                "type": "plain_text_input",
                "action_id": format!("{}_action", block_id)
            }
        })),
        InputKind::Choice => {
            if choices.is_empty() {
                warnings.push("slack.choice_without_options".into());
                return None;
            }
            let options: Vec<_> = choices
                .iter()
                .map(|choice| {
                    json!({
                        "text": plain_text(&choice.title),
                        "value": choice.value,
                    })
                })
                .collect();
            Some(json!({
                "type": "input",
                "block_id": block_id,
                "label": plain_text(label.unwrap_or("Select an option")),
                "optional": !required,
                "element": {
                    "type": "static_select",
                    "action_id": format!("{}_select", block_id),
                    "options": options
                }
            }))
        }
    }
}

fn actions_block(ir: &MessageCardIr, warnings: &mut Vec<String>) -> Option<Value> {
    if ir.actions.is_empty() {
        return None;
    }

    let mut elements = Vec::new();
    for action in &ir.actions {
        if elements.len() == BUTTON_LIMIT {
            warnings.push("slack.actions_truncated".into());
            break;
        }

        match action {
            IrAction::OpenUrl { title, url } => {
                elements.push(json!({
                    "type": "button",
                    "text": plain_text(title),
                    "url": resolve_open_url(&ir.meta, url),
                }));
            }
            IrAction::Postback { title, data } => match serde_json::to_string(data) {
                Ok(value) => {
                    elements.push(json!({
                        "type": "button",
                        "text": plain_text(title),
                        "value": value,
                        "action_id": format!("postback_{}", elements.len()),
                    }));
                }
                Err(_) => warnings.push("slack.postback_unserializable".into()),
            },
        }
    }

    if elements.is_empty() {
        None
    } else {
        Some(json!({
            "type": "actions",
            "elements": elements,
        }))
    }
}

fn section_block(text: &str, markdown: bool) -> Value {
    if markdown {
        json!({
            "type": "section",
            "text": {
                "type": "mrkdwn",
                "text": text,
            }
        })
    } else {
        json!({
            "type": "section",
            "text": {
                "type": "plain_text",
                "text": text,
            }
        })
    }
}

fn plain_text(text: &str) -> Value {
    json!({
        "type": "plain_text",
        "text": text,
        "emoji": true,
    })
}

fn truncate(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_string();
    }
    value.chars().take(limit - 1).collect::<String>() + "…"
}
