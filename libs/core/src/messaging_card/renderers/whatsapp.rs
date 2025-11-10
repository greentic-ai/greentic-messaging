use serde_json::{Map, Value, json};
use tracing::warn;

use crate::messaging_card::ir::{Element, InputKind, IrAction, MessageCardIr};
use crate::messaging_card::tier::Tier;

use super::{
    PlatformRenderer, RenderMetrics, RenderOutput, WHATSAPP_TEXT_LIMIT, enforce_text_limit,
    resolve_url_with_policy, sanitize_text_for_tier,
};

const MAX_BUTTONS: usize = 3;

#[derive(Default)]
pub struct WhatsAppRenderer;

impl PlatformRenderer for WhatsAppRenderer {
    fn platform(&self) -> &'static str {
        "whatsapp"
    }

    fn target_tier(&self) -> Tier {
        Tier::Basic
    }

    fn render(&self, ir: &MessageCardIr) -> RenderOutput {
        let mut warnings = Vec::new();
        let mut metrics = RenderMetrics::default();
        let mut body_lines = Vec::new();

        if let Some(title) = &ir.head.title {
            let sanitized = sanitize_text_for_tier(title, ir.tier, &mut metrics);
            body_lines.push(sanitized.trim().to_string());
        }
        if let Some(text) = &ir.head.text
            && !text.trim().is_empty()
        {
            let sanitized = sanitize_text_for_tier(text, ir.tier, &mut metrics);
            body_lines.push(sanitized.trim().to_string());
        }

        let primary_text = ir.head.text.as_deref().map(str::to_string);
        let mut skipped_primary = false;

        for element in &ir.elements {
            match element {
                Element::Text { text, .. } => {
                    if !skipped_primary {
                        if let Some(primary) = &primary_text
                            && primary == text
                        {
                            skipped_primary = true;
                            continue;
                        }
                        skipped_primary = true;
                    }
                    let sanitized = sanitize_text_for_tier(text, ir.tier, &mut metrics);
                    body_lines.push(sanitized.trim().to_string());
                }
                Element::Image { url, .. } => body_lines.push(url.to_string()),
                Element::FactSet { facts } => {
                    for fact in facts {
                        let label = sanitize_text_for_tier(&fact.label, ir.tier, &mut metrics);
                        let value = sanitize_text_for_tier(&fact.value, ir.tier, &mut metrics);
                        body_lines.push(format!("• {label}: {value}"));
                    }
                }
                Element::Input {
                    label,
                    kind,
                    choices,
                    ..
                } => {
                    warnings.push("whatsapp.inputs_not_supported".into());
                    warn!(
                        target = "gsm.mcard.whatsapp",
                        "downgrading inputs to prompt text"
                    );
                    let field = label
                        .as_deref()
                        .map(|value| sanitize_text_for_tier(value, ir.tier, &mut metrics))
                        .unwrap_or_else(|| "Input".into());
                    let field = field.trim().to_string();
                    let prompt = match kind {
                        InputKind::Text => format!("{field}: reply with your answer."),
                        InputKind::Choice => {
                            let opts = if choices.is_empty() {
                                "(choose any option)".to_string()
                            } else {
                                choices
                                    .iter()
                                    .map(|c| {
                                        sanitize_text_for_tier(&c.title, ir.tier, &mut metrics)
                                            .trim()
                                            .to_string()
                                    })
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            };
                            format!("{field}: reply with [{opts}].")
                        }
                    };
                    body_lines.push(prompt);
                }
            }
        }

        if let Some(footer) = &ir.head.footer {
            let sanitized = sanitize_text_for_tier(footer, ir.tier, &mut metrics);
            body_lines.push(sanitized.trim().to_string());
        }

        let mut components = Vec::new();
        let formatted_text = body_lines.join("\n");
        let text = enforce_text_limit(
            &formatted_text,
            WHATSAPP_TEXT_LIMIT,
            "whatsapp.body_truncated",
            &mut metrics,
            &mut warnings,
        );

        let buttons = build_buttons(ir, &mut warnings, &mut metrics);
        if !buttons.is_empty() {
            components.push(json!({
            "type": "BUTTONS",
            "buttons": buttons,
            }));
        }

        let mut payload = Map::new();
        payload.insert("type".into(), Value::String("WhatsAppTemplate".into()));
        payload.insert("body".into(), Value::String(text));
        if !components.is_empty() {
            payload.insert("components".into(), Value::Array(components));
        }

        let mut render_output = RenderOutput::new(Value::Object(payload));
        render_output.warnings = warnings;
        render_output.limit_exceeded = metrics.limit_exceeded;
        render_output.sanitized_count = metrics.sanitized_count;
        render_output.url_blocked_count = metrics.url_blocked_count;
        render_output
    }
}

fn build_buttons(
    ir: &MessageCardIr,
    warnings: &mut Vec<String>,
    metrics: &mut RenderMetrics,
) -> Vec<Value> {
    let mut buttons = Vec::new();
    for action in &ir.actions {
        if buttons.len() == MAX_BUTTONS {
            warnings.push("whatsapp.actions_truncated".into());
            warn!(
                target = "gsm.mcard.whatsapp",
                "action buttons truncated at WhatsApp limit"
            );
            break;
        }

        match action {
            IrAction::OpenUrl { title, url } => {
                if let Some(resolved) = resolve_url_with_policy(&ir.meta, url, metrics, warnings) {
                    let button_text = sanitize_text_for_tier(title, ir.tier, metrics);
                    buttons.push(json!({
                        "type": "URL",
                        "text": button_text,
                        "url": resolved,
                    }));
                }
            }
            IrAction::Postback { title, data } => {
                let payload = serde_json::to_string(data).unwrap_or_else(|_| "{}".into());
                let button_text = sanitize_text_for_tier(title, ir.tier, metrics);
                buttons.push(json!({
                    "type": "QUICK_REPLY",
                    "text": button_text,
                    "payload": payload,
                }));
            }
        }
    }
    buttons
}
