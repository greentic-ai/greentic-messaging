use serde_json::{Value, json};
use tracing::warn;

use crate::messaging_card::ir::{Element, InputKind, MessageCardIr};
use crate::messaging_card::tier::Tier;

use super::{
    PlatformRenderer, RenderMetrics, RenderOutput, TELEGRAM_TEXT_LIMIT, enforce_text_limit,
    resolve_url_with_policy, sanitize_text_for_tier,
};

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
        let mut metrics = RenderMetrics::default();
        let mut lines: Vec<String> = Vec::new();

        if let Some(title) = &ir.head.title {
            let escaped = sanitized_html(title, ir.tier, &mut metrics);
            if !escaped.is_empty() {
                lines.push(format!("<b>{escaped}</b>"));
            }
        }
        if let Some(text) = &ir.head.text {
            let escaped = sanitized_html(text, ir.tier, &mut metrics);
            if !escaped.is_empty() {
                lines.push(escaped);
            }
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
                    let escaped = sanitized_html(text, ir.tier, &mut metrics);
                    if !escaped.is_empty() {
                        lines.push(escaped);
                    }
                }
                Element::Image { url, .. } => lines.push(url.clone()),
                Element::FactSet { facts } => {
                    for fact in facts {
                        let label = sanitized_html(&fact.label, ir.tier, &mut metrics);
                        let value = sanitized_html(&fact.value, ir.tier, &mut metrics);
                        lines.push(format!("• <b>{label}</b>: {value}"));
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
                    let prompt = label
                        .as_deref()
                        .map(|value| sanitize_text_for_tier(value, ir.tier, &mut metrics))
                        .unwrap_or_else(|| "Input".into());
                    let prompt_escaped = html_escape(prompt.trim());
                    let prompt_text = match kind {
                        InputKind::Text => {
                            format!("<i>{prompt_escaped}</i>: reply with your answer.")
                        }
                        InputKind::Choice => {
                            let opts = if choices.is_empty() {
                                "(any option)".to_string()
                            } else {
                                choices
                                    .iter()
                                    .map(|c| {
                                        let sanitized =
                                            sanitize_text_for_tier(&c.title, ir.tier, &mut metrics);
                                        html_escape(sanitized.trim())
                                    })
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            };
                            format!("<i>{prompt_escaped}</i>: reply with one of [{opts}].")
                        }
                    };
                    lines.push(prompt_text);
                }
            }
        }

        if let Some(footer) = &ir.head.footer {
            let escaped = sanitized_html(footer, ir.tier, &mut metrics);
            if !escaped.is_empty() {
                lines.push(escaped);
            }
        }

        let text = lines.join("\n");
        let limited = enforce_text_limit(
            &text,
            TELEGRAM_TEXT_LIMIT,
            "telegram.body_truncated",
            &mut metrics,
            &mut warnings,
        );
        let mut payload = RenderOutput::new(json!({
            "method": "sendMessage",
            "parse_mode": "HTML",
            "text": limited,
        }));

        if let Some(keyboard) = build_keyboard(ir, &mut warnings, &mut metrics) {
            payload.payload["reply_markup"] = json!({ "inline_keyboard": keyboard });
        }

        payload.warnings = warnings;
        payload.limit_exceeded = metrics.limit_exceeded;
        payload.sanitized_count = metrics.sanitized_count;
        payload.url_blocked_count = metrics.url_blocked_count;
        payload
    }
}

fn build_keyboard(
    ir: &MessageCardIr,
    warnings: &mut Vec<String>,
    metrics: &mut RenderMetrics,
) -> Option<Vec<Vec<Value>>> {
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
                if let Some(resolved) = resolve_url_with_policy(&ir.meta, url, metrics, warnings) {
                    buttons.push(json!({
                        "text": sanitized_html(title, ir.tier, metrics),
                        "url": resolved,
                    }));
                }
            }
            super::IrAction::Postback { title, data } => {
                let data_str = serde_json::to_string(data).unwrap_or_else(|_| "{}".into());
                buttons.push(json!({
                    "text": sanitized_html(title, ir.tier, metrics),
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

fn sanitized_html(text: &str, tier: Tier, metrics: &mut RenderMetrics) -> String {
    let sanitized = sanitize_text_for_tier(text, tier, metrics);
    html_escape(sanitized.trim())
}
