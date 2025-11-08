use serde_json::{Map, Value, json};
use tracing::warn;

use crate::messaging_card::ir::{Element, InputKind, IrAction, MessageCardIr};
use crate::messaging_card::tier::Tier;

use super::{PlatformRenderer, RenderOutput, resolve_open_url};

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
        let mut body_lines = Vec::new();

        if let Some(title) = &ir.head.title {
            body_lines.push(title.trim().to_string());
        }
        if let Some(text) = &ir.head.text {
            if !text.trim().is_empty() {
                body_lines.push(text.trim().to_string());
            }
        }

        let primary_text = ir.head.text.as_deref().map(str::to_string);
        let mut skipped_primary = false;

        for element in &ir.elements {
            match element {
                Element::Text { text, .. } => {
                    if !skipped_primary {
                        if let Some(primary) = &primary_text {
                            if primary == text {
                                skipped_primary = true;
                                continue;
                            }
                        }
                        skipped_primary = true;
                    }
                    body_lines.push(text.trim().to_string());
                }
                Element::Image { url, .. } => body_lines.push(url.to_string()),
                Element::FactSet { facts } => {
                    for fact in facts {
                        body_lines.push(format!("• {}: {}", fact.label, fact.value));
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
                    let field = label.as_deref().unwrap_or("Input");
                    let prompt = match kind {
                        InputKind::Text => format!("{}: reply with your answer.", field),
                        InputKind::Choice => {
                            let opts = if choices.is_empty() {
                                "(choose any option)".to_string()
                            } else {
                                choices
                                    .iter()
                                    .map(|c| c.title.trim())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            };
                            format!("{}: reply with [{}].", field, opts)
                        }
                    };
                    body_lines.push(prompt);
                }
            }
        }

        if let Some(footer) = &ir.head.footer {
            body_lines.push(footer.trim().to_string());
        }

        let mut components = Vec::new();
        let formatted_text = body_lines.join("\n");

        let buttons = build_buttons(ir, &mut warnings);
        if !buttons.is_empty() {
            components.push(json!({
            "type": "BUTTONS",
            "buttons": buttons,
            }));
        }

        let mut payload = Map::new();
        payload.insert("type".into(), Value::String("WhatsAppTemplate".into()));
        payload.insert("body".into(), Value::String(formatted_text));
        if !components.is_empty() {
            payload.insert("components".into(), Value::Array(components));
        }

        let mut render_output = RenderOutput::new(Value::Object(payload));
        render_output.warnings = warnings;
        render_output
    }
}

fn build_buttons(ir: &MessageCardIr, warnings: &mut Vec<String>) -> Vec<Value> {
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
                buttons.push(json!({
                    "type": "URL",
                    "text": title,
                    "url": resolve_open_url(&ir.meta, url),
                }));
            }
            IrAction::Postback { title, data } => {
                let payload = serde_json::to_string(data).unwrap_or_else(|_| "{}".into());
                buttons.push(json!({
                    "type": "QUICK_REPLY",
                    "text": title,
                    "payload": payload,
                }));
            }
        }
    }
    buttons
}
