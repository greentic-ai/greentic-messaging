use serde_json::{Value, json};
use tracing::warn;

use crate::messaging_card::ir::{Element, IrAction, MessageCardIr};
use crate::messaging_card::tier::Tier;

use super::{
    PlatformRenderer, RenderMetrics, RenderOutput, WEBEX_TEXT_LIMIT, enforce_text_limit,
    resolve_url_with_policy, sanitize_text_for_tier,
};

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
        let mut metrics = RenderMetrics::default();
        let mut body = Vec::new();

        if let Some(title) = &ir.head.title {
            let sanitized = sanitize_text_for_tier(title, ir.tier, &mut metrics);
            body.push(primary_text_block(&enforce_text_limit(
                &sanitized,
                WEBEX_TEXT_LIMIT,
                "webex.text_truncated",
                &mut metrics,
                &mut warnings,
            )));
        }

        if let Some(subtitle) = &ir.head.text {
            let sanitized = sanitize_text_for_tier(subtitle, ir.tier, &mut metrics);
            body.push(subtle_text_block(&enforce_text_limit(
                &sanitized,
                WEBEX_TEXT_LIMIT,
                "webex.text_truncated",
                &mut metrics,
                &mut warnings,
            )));
        }

        for element in &ir.elements {
            match element {
                Element::Text { text, .. } => {
                    let sanitized = sanitize_text_for_tier(text, ir.tier, &mut metrics);
                    body.push(text_block(&enforce_text_limit(
                        &sanitized,
                        WEBEX_TEXT_LIMIT,
                        "webex.text_truncated",
                        &mut metrics,
                        &mut warnings,
                    )));
                }
                Element::Image { url, alt } => {
                    let alt_text = alt
                        .as_deref()
                        .map(|value| sanitize_text_for_tier(value, ir.tier, &mut metrics))
                        .unwrap_or_else(|| "image".into());
                    body.push(json!({
                        "type": "Image",
                        "url": url,
                        "altText": alt_text,
                    }));
                }
                Element::FactSet { facts } => {
                    if facts.is_empty() {
                        continue;
                    }
                    let lines: Vec<String> = facts
                        .iter()
                        .map(|fact| {
                            let label = sanitize_text_for_tier(&fact.label, ir.tier, &mut metrics);
                            let value = sanitize_text_for_tier(&fact.value, ir.tier, &mut metrics);
                            format!("*{label}*: {value}")
                        })
                        .collect();
                    let text = lines.join("\n");
                    body.push(text_block(&enforce_text_limit(
                        &text,
                        WEBEX_TEXT_LIMIT,
                        "webex.text_truncated",
                        &mut metrics,
                        &mut warnings,
                    )));
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
            let sanitized = sanitize_text_for_tier(footer, ir.tier, &mut metrics);
            body.push(subtle_footer_block(&enforce_text_limit(
                &sanitized,
                WEBEX_TEXT_LIMIT,
                "webex.text_truncated",
                &mut metrics,
                &mut warnings,
            )));
        }

        let mut actions = Vec::new();
        for action in &ir.actions {
            match action {
                IrAction::OpenUrl { title, url } => {
                    if let Some(resolved) =
                        resolve_url_with_policy(&ir.meta, url, &mut metrics, &mut warnings)
                    {
                        let sanitized = sanitize_text_for_tier(title, ir.tier, &mut metrics);
                        actions.push(json!({
                            "type": "Action.OpenUrl",
                            "title": sanitized,
                            "url": resolved,
                        }));
                    }
                }
                IrAction::Postback { title, data } => {
                    let sanitized = sanitize_text_for_tier(title, ir.tier, &mut metrics);
                    actions.push(json!({
                        "type": "Action.Submit",
                        "title": sanitized,
                        "data": data,
                    }));
                }
            }
        }

        let payload = json!({
            "type": "AdaptiveCard",
            "version": "1.4",
            "body": body,
            "actions": actions,
        });

        let mut output = RenderOutput::new(payload);
        output.warnings = warnings;
        output.limit_exceeded = metrics.limit_exceeded;
        output.sanitized_count = metrics.sanitized_count;
        output.url_blocked_count = metrics.url_blocked_count;
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

fn primary_text_block(text: &str) -> Value {
    let mut block = text_block(text);
    block["weight"] = json!("Bolder");
    block["size"] = json!("Medium");
    block
}

fn subtle_text_block(text: &str) -> Value {
    let mut block = text_block(text);
    block["isSubtle"] = json!(true);
    block
}

fn subtle_footer_block(text: &str) -> Value {
    let mut block = subtle_text_block(text);
    block["spacing"] = json!("Small");
    block["size"] = json!("Small");
    block
}
