use std::collections::BTreeMap;
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::Sha256;
use urlencoding::encode;

use crate::messaging_card::ir::{Element, InputChoice, InputKind, IrAction, MessageCardIr, Meta};
use crate::messaging_card::tier::Tier;

mod slack;
mod teams;
mod telegram;
mod webchat;
mod webex;
mod whatsapp;

pub use slack::SlackRenderer;
pub use teams::TeamsRenderer;
pub use telegram::TelegramRenderer;
pub use webchat::WebChatRenderer;
pub use webex::WebexRenderer;
pub use whatsapp::WhatsAppRenderer;

const ADAPTIVE_SCHEMA: &str = "http://adaptivecards.io/schemas/adaptive-card.json";
const ADAPTIVE_VERSION: &str = "1.6";

pub trait PlatformRenderer: Send + Sync {
    fn platform(&self) -> &'static str;
    fn target_tier(&self) -> Tier;
    fn render(&self, ir: &MessageCardIr) -> RenderOutput;
}

#[derive(Debug, Clone)]
pub struct RenderOutput {
    pub payload: Value,
    pub used_modal: bool,
    pub warnings: Vec<String>,
}

impl RenderOutput {
    pub fn new(payload: Value) -> Self {
        Self {
            payload,
            used_modal: false,
            warnings: Vec::new(),
        }
    }
}

#[derive(Default)]
pub struct RendererRegistry {
    renderers: BTreeMap<String, Arc<dyn PlatformRenderer>>,
}

impl RendererRegistry {
    pub fn register<R>(&mut self, renderer: R)
    where
        R: PlatformRenderer + 'static,
    {
        self.renderers
            .insert(renderer.platform().to_string(), Arc::new(renderer));
    }

    pub fn get(&self, platform: &str) -> Option<Arc<dyn PlatformRenderer>> {
        self.renderers.get(platform).cloned()
    }

    pub fn render(&self, platform: &str, ir: &MessageCardIr) -> Option<RenderOutput> {
        self.get(platform).map(|renderer| renderer.render(ir))
    }

    pub fn platforms(&self) -> Vec<String> {
        self.renderers.keys().cloned().collect()
    }
}

/// Bootstrap renderer that simply exposes the IR as JSON.
pub struct NullRenderer;

impl PlatformRenderer for NullRenderer {
    fn platform(&self) -> &'static str {
        "null"
    }

    fn target_tier(&self) -> Tier {
        Tier::Basic
    }

    fn render(&self, ir: &MessageCardIr) -> RenderOutput {
        RenderOutput::new(json!({
            "platform": self.platform(),
            "tier": ir.tier.as_str(),
            "head": ir.head,
            "elements": ir.elements,
            "actions": ir.actions,
            "meta": ir.meta,
        }))
    }
}

pub fn adaptive_from_ir(ir: &MessageCardIr) -> Value {
    if let Some(raw) = &ir.meta.adaptive_payload {
        return raw.clone();
    }

    let mut body = Vec::new();
    if let Some(title) = &ir.head.title {
        body.push(json!({
            "type": "TextBlock",
            "text": title,
            "wrap": true,
            "weight": "Bolder"
        }));
    }

    let has_text_block = ir
        .elements
        .iter()
        .any(|el| matches!(el, Element::Text { .. }));
    if !has_text_block {
        if let Some(text) = &ir.head.text {
            body.push(json!({
                "type": "TextBlock",
                "text": text,
                "wrap": true
            }));
        }
    }

    let mut input_index = 0usize;
    for element in &ir.elements {
        match element {
            Element::Text { text, markdown } => {
                body.push(json!({
                    "type": "TextBlock",
                    "text": text,
                    "wrap": true,
                    "isSubtle": !markdown,
                }));
            }
            Element::Image { url, alt } => {
                let mut image = json!({
                    "type": "Image",
                    "url": url,
                });
                if let Some(alt) = alt {
                    image["altText"] = json!(alt);
                }
                body.push(image);
            }
            Element::FactSet { facts } => {
                if !facts.is_empty() {
                    let facts_json: Vec<_> = facts
                        .iter()
                        .map(|fact| {
                            json!({
                                "title": fact.label,
                                "value": fact.value,
                            })
                        })
                        .collect();
                    body.push(json!({
                        "type": "FactSet",
                        "facts": facts_json,
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
                let resolved_id = id
                    .clone()
                    .unwrap_or_else(|| format!("input_{}", input_index));
                input_index += 1;

                match kind {
                    InputKind::Text => {
                        let mut input = json!({
                            "type": "Input.Text",
                            "id": resolved_id,
                            "isRequired": required,
                        });
                        if let Some(label) = label {
                            input["label"] = json!(label);
                        }
                        body.push(input);
                    }
                    InputKind::Choice => {
                        if choices.is_empty() {
                            let mut input = json!({
                                "type": "Input.Text",
                                "id": resolved_id,
                                "isRequired": required,
                            });
                            if let Some(label) = label {
                                input["label"] = json!(label);
                            }
                            body.push(input);
                        } else {
                            body.push(render_choice_input(
                                label.clone(),
                                resolved_id,
                                *required,
                                choices,
                            ));
                        }
                    }
                }
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

    let actions = render_actions(ir);

    json!({
        "type": "AdaptiveCard",
        "$schema": ADAPTIVE_SCHEMA,
        "version": ADAPTIVE_VERSION,
        "body": body,
        "actions": actions,
    })
}

fn render_choice_input(
    label: Option<String>,
    id: String,
    required: bool,
    choices: &[InputChoice],
) -> Value {
    let rendered_choices: Vec<_> = choices
        .iter()
        .map(|choice| {
            json!({
                "title": choice.title,
                "value": choice.value,
            })
        })
        .collect();

    let mut input = json!({
        "type": "Input.ChoiceSet",
        "id": id,
        "choices": rendered_choices,
        "style": "compact",
        "isRequired": required,
    });
    if let Some(label) = label {
        input["label"] = json!(label);
    }
    input
}

fn render_actions(ir: &MessageCardIr) -> Vec<Value> {
    ir.actions
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
        .collect()
}

pub(crate) fn resolve_open_url(meta: &Meta, url: &str) -> String {
    match &meta.app_link {
        Some(app_link) => build_signed_link(app_link, url).unwrap_or_else(|| url.to_string()),
        None => url.to_string(),
    }
}

fn build_signed_link(
    app_link: &crate::messaging_card::ir::AppLink,
    target: &str,
) -> Option<String> {
    let mut base = app_link
        .base_url
        .trim_end_matches('&')
        .trim_end_matches('?')
        .to_string();
    if base.is_empty() {
        return None;
    }
    let encoded_target = encode(target);
    base = append_query(base, "target", &encoded_target);
    if let Some(secret) = &app_link.secret {
        type HmacSha256 = Hmac<Sha256>;
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).ok()?;
        mac.update(target.as_bytes());
        let signature = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
        base = append_query(base, "sig", &signature);
    }
    Some(base)
}

fn append_query(mut base: String, key: &str, value: &str) -> String {
    if !base.contains('?') {
        base.push('?');
    } else if !base.ends_with('?') && !base.ends_with('&') {
        base.push('&');
    }
    base.push_str(key);
    base.push('=');
    base.push_str(value);
    base
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messaging_card::ir::MessageCardIrBuilder;

    #[test]
    fn registry_returns_registered_renderer() {
        let mut registry = RendererRegistry::default();
        registry.register(NullRenderer);
        assert_eq!(registry.platforms(), vec!["null".to_string()]);
        let ir = MessageCardIrBuilder::default().tier(Tier::Basic).build();
        let rendered = registry
            .render("null", &ir)
            .expect("renderer must exist for bootstrap");
        assert_eq!(rendered.payload["platform"], "null");
    }
}
