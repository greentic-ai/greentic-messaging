use serde_json::{Value, json};
use tracing::warn;

use crate::messaging_card::ir::{Element, InputKind, MessageCardIr};
use crate::messaging_card::tier::Tier;

use super::{PlatformRenderer, RenderOutput, resolve_open_url};

const MAX_BUTTONS: usize = 10;
const MAX_PER_ROW: usize = 3;

#[derive(Default)]
pub struct TelegramRenderer;

impl PlatformRenderer for TelegramRenderer {
    fn platform(&self) -> &'static str {
        "telegram"
    }

    fn target_tier(&self) -> Tier {
        Tier::Basic
    }

    fn render(&self, ir: &MessageCardIr) -> RenderOutput {
        let mut warnings = Vec::new();
        let mut lines: Vec<String> = Vec::new();

        if let Some(title) = &ir.head.title {
            lines.push(format!("<b>{}</b>", html_escape(title)));
        }
        if let Some(text) = &ir.head.text {
            lines.push(html_escape(text));
        }

        let mut primary_consumed = ir.head.text.is_none();

        for element in &ir.elements {
            match element {
                Element::Text { text, .. } => {
                    if !primary_consumed && ir.head.text.as_deref() == Some(text.as_str()) {
                        primary_consumed = true;
                        continue;
                    }
                    primary_consumed = true;
                    lines.push(html_escape(text));
                }
                Element::Image { url, .. } => lines.push(url.clone()),
                Element::FactSet { facts } => {
                    for fact in facts {
                        lines.push(format!(
                            "• <b>{}</b>: {}",
                            html_escape(&fact.label),
                            html_escape(&fact.value)
                        ));
                    }
                }
                Element::Input {
                    label,
                    kind,
                    choices,
                    ..
                } => {
                    warnings.push("telegram.inputs_not_supported".into());
                    warn!(
                        target = "gsm.mcard.telegram",
                        "inputs not supported on Telegram"
                    );
                    let prompt = label.as_deref().unwrap_or("Input");
                    let prompt_text = match kind {
                        InputKind::Text => {
                            format!("<i>{}</i>: reply with your answer.", html_escape(prompt))
                        }
                        InputKind::Choice => {
                            let opts = if choices.is_empty() {
                                "(any option)".to_string()
                            } else {
                                choices
                                    .iter()
                                    .map(|c| html_escape(&c.title))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            };
                            format!(
                                "<i>{}</i>: reply with one of [{}].",
                                html_escape(prompt),
                                opts
                            )
                        }
                    };
                    lines.push(prompt_text);
                }
            }
        }

        if let Some(footer) = &ir.head.footer {
            lines.push(html_escape(footer));
        }

        let text = lines.join("\n");
        let mut payload = RenderOutput::new(json!({
            "method": "sendMessage",
            "parse_mode": "HTML",
            "text": text,
        }));

        if let Some(keyboard) = build_keyboard(ir, &mut warnings) {
            payload.payload["reply_markup"] = json!({ "inline_keyboard": keyboard });
        }

        payload.warnings = warnings;
        payload
    }
}

fn build_keyboard(ir: &MessageCardIr, warnings: &mut Vec<String>) -> Option<Vec<Vec<Value>>> {
    if ir.actions.is_empty() {
        return None;
    }

    let mut buttons = Vec::new();
    for action in &ir.actions {
        if buttons.len() == MAX_BUTTONS {
            warnings.push("telegram.actions_truncated".into());
            warn!(
                target = "gsm.mcard.telegram",
                "actions truncated at Telegram limit"
            );
            break;
        }
        match action {
            super::IrAction::OpenUrl { title, url } => {
                buttons.push(json!({
                    "text": html_escape(title),
                    "url": resolve_open_url(&ir.meta, url),
                }));
            }
            super::IrAction::Postback { title, data } => {
                let data_str = serde_json::to_string(data).unwrap_or_else(|_| "{}".into());
                buttons.push(json!({
                    "text": html_escape(title),
                    "callback_data": data_str,
                }));
            }
        }
    }

    if buttons.is_empty() {
        None
    } else {
        let mut rows = Vec::new();
        for chunk in buttons.chunks(MAX_PER_ROW) {
            rows.push(chunk.to_vec());
        }
        Some(rows)
    }
}

fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
