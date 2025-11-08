use serde_json::{Value, json};

use crate::messaging_card::ir::{Element, InputChoice, InputKind, IrAction, MessageCardIr};
use crate::messaging_card::tier::Tier;

use super::{
    PlatformRenderer, RenderMetrics, RenderOutput, SLACK_TEXT_LIMIT, enforce_text_limit,
    resolve_url_with_policy, sanitize_text_for_tier,
};

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
        let mut metrics = RenderMetrics::default();
        let has_inputs = ir
            .elements
            .iter()
            .any(|el| matches!(el, Element::Input { .. }));

        let payload = if has_inputs {
            render_modal(ir, &mut warnings, &mut metrics)
        } else {
            json!({ "blocks": render_blocks(ir, &mut warnings, false, &mut metrics) })
        };
        let mut output = RenderOutput::new(payload);
        output.used_modal = has_inputs;
        output.warnings = warnings;
        output.limit_exceeded = metrics.limit_exceeded;
        output.sanitized_count = metrics.sanitized_count;
        output.url_blocked_count = metrics.url_blocked_count;
        output
    }
}

fn render_modal(
    ir: &MessageCardIr,
    warnings: &mut Vec<String>,
    metrics: &mut RenderMetrics,
) -> Value {
    let title_raw = ir
        .head
        .title
        .as_deref()
        .or(ir.head.text.as_deref())
        .unwrap_or("Card");
    let sanitized = sanitize_text_for_tier(title_raw, ir.tier, metrics);
    let title = truncate(&sanitized, MODAL_TITLE_LIMIT);

    json!({
        "type": "modal",
        "title": plain_text(&title),
        "submit": plain_text("Submit"),
        "close": plain_text("Close"),
        "callback_id": "gsm_card_modal",
        "blocks": render_blocks(ir, warnings, true, metrics),
    })
}

fn render_blocks(
    ir: &MessageCardIr,
    warnings: &mut Vec<String>,
    include_inputs: bool,
    metrics: &mut RenderMetrics,
) -> Vec<Value> {
    let mut blocks = Vec::new();

    if !include_inputs && let Some(title) = &ir.head.title {
        let sanitized = sanitize_text_for_tier(title, ir.tier, metrics);
        let limited = enforce_text_limit(
            &sanitized,
            SLACK_TEXT_LIMIT,
            "slack.text_truncated",
            metrics,
            warnings,
        );
        blocks.push(json!({
            "type": "header",
            "text": plain_text(&truncate(&limited, HEADER_LIMIT)),
        }));
    }

    let mut saw_text_element = false;

    for element in &ir.elements {
        match element {
            Element::Text { text, markdown } => {
                saw_text_element = true;
                let sanitized = sanitize_text_for_tier(text, ir.tier, metrics);
                let limited = enforce_text_limit(
                    &sanitized,
                    SLACK_TEXT_LIMIT,
                    "slack.text_truncated",
                    metrics,
                    warnings,
                );
                blocks.push(section_block(&limited, *markdown));
            }
            Element::Image { url, alt } => {
                let alt_text = alt
                    .as_deref()
                    .map(|value| sanitize_text_for_tier(value, ir.tier, metrics))
                    .unwrap_or_else(|| "image".to_string());
                blocks.push(json!({
                    "type": "image",
                    "image_url": url,
                    "alt_text": alt_text,
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
                    let label = sanitize_text_for_tier(&fact.label, ir.tier, metrics);
                    let value = sanitize_text_for_tier(&fact.value, ir.tier, metrics);
                    let text = format!("*{}*\n{}", label, value);
                    fields.push(json!({
                        "type": "mrkdwn",
                        "text": enforce_text_limit(
                            &text,
                            SLACK_TEXT_LIMIT,
                            "slack.text_truncated",
                            metrics,
                            warnings,
                        )
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
                        metrics,
                        ir.tier,
                    ) {
                        blocks.push(block);
                    }
                } else {
                    warnings.push("slack.inputs_require_modal".into());
                }
            }
        }
    }

    if !saw_text_element && let Some(text) = &ir.head.text {
        let sanitized = sanitize_text_for_tier(text, ir.tier, metrics);
        let limited = enforce_text_limit(
            &sanitized,
            SLACK_TEXT_LIMIT,
            "slack.text_truncated",
            metrics,
            warnings,
        );
        blocks.push(section_block(&limited, true));
    }

    if let Some(footer) = &ir.head.footer {
        blocks.push(json!({
            "type": "context",
            "elements": [
                json!({
                    "type": "mrkdwn",
                    "text": enforce_text_limit(
                        &sanitize_text_for_tier(footer, ir.tier, metrics),
                        SLACK_TEXT_LIMIT,
                        "slack.text_truncated",
                        metrics,
                        warnings,
                    ),
                })
            ],
        }));
    }

    if let Some(actions) = actions_block(ir, warnings, metrics) {
        blocks.push(actions);
    }

    blocks
}

#[allow(clippy::too_many_arguments)]
fn input_block(
    label: Option<&str>,
    kind: &InputKind,
    id: Option<&str>,
    required: bool,
    choices: &[InputChoice],
    warnings: &mut Vec<String>,
    metrics: &mut RenderMetrics,
    tier: Tier,
) -> Option<Value> {
    let block_id = id.unwrap_or("input").to_string();
    match kind {
        InputKind::Text => Some(json!({
            "type": "input",
            "block_id": block_id,
            "label": plain_text(&sanitize_text_for_tier(label.unwrap_or("Input"), tier, metrics)),
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
                        "text": plain_text(&sanitize_text_for_tier(&choice.title, tier, metrics)),
                        "value": choice.value,
                    })
                })
                .collect();
            Some(json!({
                "type": "input",
                "block_id": block_id,
                "label": plain_text(&sanitize_text_for_tier(
                    label.unwrap_or("Select an option"),
                    tier,
                    metrics,
                )),
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

fn actions_block(
    ir: &MessageCardIr,
    warnings: &mut Vec<String>,
    metrics: &mut RenderMetrics,
) -> Option<Value> {
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
                if let Some(resolved) = resolve_url_with_policy(&ir.meta, url, metrics, warnings) {
                    let button_text = sanitize_text_for_tier(title, ir.tier, metrics);
                    elements.push(json!({
                        "type": "button",
                        "text": plain_text(&button_text),
                        "url": resolved,
                    }));
                }
            }
            IrAction::Postback { title, data } => match serde_json::to_string(data) {
                Ok(value) => {
                    let button_text = sanitize_text_for_tier(title, ir.tier, metrics);
                    elements.push(json!({
                        "type": "button",
                        "text": plain_text(&button_text),
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
