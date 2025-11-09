use std::collections::BTreeMap;
use std::env;
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{Hmac, Mac};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use once_cell::sync::Lazy;
use regex::Regex;
use serde::Serialize;
use serde_json::{Value, json};
use sha2::Sha256;
use tracing::warn;
use urlencoding::encode;

use crate::messaging_card::ir::{
    AppLink, AppLinkJwt, Element, InputChoice, InputKind, IrAction, MessageCardIr, Meta,
};
use crate::messaging_card::spec::AuthRenderSpec;
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
const SLACK_TEXT_LIMIT: usize = 3000;
const WEBEX_TEXT_LIMIT: usize = 3000;
const TELEGRAM_TEXT_LIMIT: usize = 4000;
const WHATSAPP_TEXT_LIMIT: usize = 4000;
const MAX_STATE_BYTES: usize = 2048;

pub trait PlatformRenderer: Send + Sync {
    fn platform(&self) -> &'static str;
    fn target_tier(&self) -> Tier;
    fn render(&self, ir: &MessageCardIr) -> RenderOutput;

    fn render_auth(&self, _auth: &AuthRenderSpec) -> Option<RenderOutput> {
        None
    }
}

#[derive(Debug, Clone)]
pub struct RenderOutput {
    pub payload: Value,
    pub used_modal: bool,
    pub warnings: Vec<String>,
    pub limit_exceeded: bool,
    pub sanitized_count: usize,
    pub url_blocked_count: usize,
}

impl RenderOutput {
    pub fn new(payload: Value) -> Self {
        Self {
            payload,
            used_modal: false,
            warnings: Vec::new(),
            limit_exceeded: false,
            sanitized_count: 0,
            url_blocked_count: 0,
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct RenderMetrics {
    pub sanitized_count: usize,
    pub url_blocked_count: usize,
    pub limit_exceeded: bool,
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

    pub fn render_auth(&self, platform: &str, auth: &AuthRenderSpec) -> Option<RenderOutput> {
        self.get(platform)
            .and_then(|renderer| renderer.render_auth(auth))
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

pub fn adaptive_from_ir(
    ir: &MessageCardIr,
    metrics: &mut RenderMetrics,
    warnings: &mut Vec<String>,
) -> Value {
    if let Some(raw) = &ir.meta.adaptive_payload {
        return raw.clone();
    }

    let mut body = Vec::new();
    if let Some(title) = &ir.head.title {
        let sanitized = sanitize_text_for_tier(title, ir.tier, metrics);
        body.push(json!({
            "type": "TextBlock",
            "text": sanitized,
            "wrap": true,
            "weight": "Bolder"
        }));
    }

    let has_text_block = ir
        .elements
        .iter()
        .any(|el| matches!(el, Element::Text { .. }));
    if !has_text_block && let Some(text) = &ir.head.text {
        let sanitized = sanitize_text_for_tier(text, ir.tier, metrics);
        body.push(json!({
            "type": "TextBlock",
            "text": sanitized,
            "wrap": true
        }));
    }

    let mut input_index = 0usize;
    for element in &ir.elements {
        match element {
            Element::Text { text, markdown } => {
                let sanitized = sanitize_text_for_tier(text, ir.tier, metrics);
                body.push(json!({
                    "type": "TextBlock",
                    "text": sanitized,
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
                            let label = sanitize_text_for_tier(&fact.label, ir.tier, metrics);
                            let value = sanitize_text_for_tier(&fact.value, ir.tier, metrics);
                            json!({
                                "title": label,
                                "value": value,
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
                        let sanitized_label = label
                            .as_deref()
                            .map(|l| sanitize_text_for_tier(l, ir.tier, metrics));
                        let mut input = json!({
                            "type": "Input.Text",
                            "id": resolved_id,
                            "isRequired": required,
                        });
                        if let Some(label) = sanitized_label {
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
                                let sanitized = sanitize_text_for_tier(label, ir.tier, metrics);
                                input["label"] = json!(sanitized);
                            }
                            body.push(input);
                        } else {
                            body.push(render_choice_input(
                                label.clone(),
                                resolved_id,
                                *required,
                                choices,
                                ir.tier,
                                metrics,
                            ));
                        }
                    }
                }
            }
        }
    }

    if let Some(footer) = &ir.head.footer {
        let sanitized = sanitize_text_for_tier(footer, ir.tier, metrics);
        body.push(json!({
            "type": "TextBlock",
            "text": sanitized,
            "wrap": true,
            "spacing": "Small",
            "isSubtle": true,
            "size": "Small",
        }));
    }

    let mut actions = render_actions(ir, metrics, warnings);
    enforce_payload_limit(
        &mut body,
        &mut actions,
        25 * 1024,
        metrics,
        warnings,
        "adaptive.payload_truncated",
    );

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
    tier: Tier,
    metrics: &mut RenderMetrics,
) -> Value {
    let rendered_choices: Vec<_> = choices
        .iter()
        .map(|choice| {
            let sanitized = sanitize_text_for_tier(&choice.title, tier, metrics);
            json!({
                "title": sanitized,
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
        let sanitized = sanitize_text_for_tier(&label, tier, metrics);
        input["label"] = json!(sanitized);
    }
    input
}

fn render_actions(
    ir: &MessageCardIr,
    metrics: &mut RenderMetrics,
    warnings: &mut Vec<String>,
) -> Vec<Value> {
    let mut rendered = Vec::new();
    for action in &ir.actions {
        match action {
            IrAction::OpenUrl { title, url } => {
                if let Some(resolved) = resolve_url_with_policy(&ir.meta, url, metrics, warnings) {
                    rendered.push(json!({
                        "type": "Action.OpenUrl",
                        "title": title,
                        "url": resolved,
                    }));
                }
            }
            IrAction::Postback { title, data } => {
                rendered.push(json!({
                    "type": "Action.Submit",
                    "title": title,
                    "data": data,
                }));
            }
        }
    }
    rendered
}

pub(crate) fn resolve_open_url(meta: &Meta, url: &str) -> String {
    match &meta.app_link {
        Some(app_link) => build_signed_link(app_link, url).unwrap_or_else(|| url.to_string()),
        None => url.to_string(),
    }
}

fn build_signed_link(app_link: &AppLink, target: &str) -> Option<String> {
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

    if let Some(jwt_cfg) = &app_link.jwt
        && let Some(token) = encode_app_state_jwt(app_link, jwt_cfg, target)
    {
        base = append_query(base, "state_jwt", &token);
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

fn encode_app_state_jwt(app_link: &AppLink, cfg: &AppLinkJwt, target: &str) -> Option<String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let exp = now + cfg.ttl_seconds as i64;
    let state = app_link
        .state
        .as_ref()
        .and_then(|value| normalize_state_payload(value));
    let claims = AppStateClaims {
        iat: now,
        exp,
        aud: cfg.audience.as_deref(),
        iss: cfg.issuer.as_deref(),
        scope: app_link.scope.as_deref(),
        tenant: app_link.tenant.as_deref(),
        target,
        state,
    };

    let algorithm = match cfg.algorithm.to_uppercase().as_str() {
        "HS384" => Algorithm::HS384,
        "HS512" => Algorithm::HS512,
        _ => Algorithm::HS256,
    };
    let key = EncodingKey::from_secret(cfg.secret.as_bytes());
    match jsonwebtoken::encode(&Header::new(algorithm), &claims, &key) {
        Ok(token) => Some(token),
        Err(err) => {
            warn!(
                target = "gsm.mcard.render",
                "failed to encode app state jwt: {err}"
            );
            None
        }
    }
}

#[derive(Serialize)]
struct AppStateClaims<'a> {
    iat: i64,
    exp: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    aud: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    iss: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    scope: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tenant: Option<&'a str>,
    target: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    state: Option<&'a Value>,
}

fn normalize_state_payload(value: &Value) -> Option<&Value> {
    match value {
        Value::Object(_) | Value::Array(_) => match serde_json::to_vec(value) {
            Ok(serialized) if serialized.len() <= MAX_STATE_BYTES => Some(value),
            Ok(_) => {
                warn!(
                    target = "gsm.mcard.render",
                    "app_link state payload exceeds {} bytes; dropping state", MAX_STATE_BYTES
                );
                None
            }
            Err(err) => {
                warn!(
                    target = "gsm.mcard.render",
                    "failed to serialize app_link state: {err}"
                );
                None
            }
        },
        Value::Null => None,
        _ => {
            warn!(
                target = "gsm.mcard.render",
                "app_link state must be an object or array"
            );
            None
        }
    }
}

static TAG_REGEX: Lazy<Regex> = Lazy::new(|| Regex::new(r"<[^>]+>").unwrap());
static URL_ALLOW_LIST: Lazy<RwLock<Option<Vec<String>>>> =
    Lazy::new(|| RwLock::new(read_allow_list_from_env()));

fn read_allow_list_from_env() -> Option<Vec<String>> {
    env::var("CARD_URL_ALLOW_LIST").ok().map(|v| {
        v.split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
    })
}

fn sanitize_text_for_tier(text: &str, tier: Tier, metrics: &mut RenderMetrics) -> String {
    if matches!(tier, Tier::Premium) {
        return text.to_string();
    }
    let cleaned = TAG_REGEX.replace_all(text, "");
    let normalized = cleaned.replace(['\u{2028}', '\u{2029}'], " ");
    let sanitized = normalized.trim().to_string();
    if sanitized != text {
        metrics.sanitized_count += 1;
    }
    sanitized
}

fn enforce_text_limit(
    text: &str,
    limit: usize,
    warning: &str,
    metrics: &mut RenderMetrics,
    warnings: &mut Vec<String>,
) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    metrics.limit_exceeded = true;
    warnings.push(warning.into());
    text.chars().take(limit).collect()
}

fn resolve_url_with_policy(
    meta: &Meta,
    url: &str,
    metrics: &mut RenderMetrics,
    warnings: &mut Vec<String>,
) -> Option<String> {
    let resolved = resolve_open_url(meta, url);
    if !is_url_allowed(&resolved) {
        metrics.url_blocked_count += 1;
        warnings.push("url_blocked".into());
        return None;
    }
    Some(resolved)
}

fn is_url_allowed(url: &str) -> bool {
    let guard = URL_ALLOW_LIST.read().expect("url allow list poisoned");
    match &*guard {
        Some(list) => list.iter().any(|prefix| url.starts_with(prefix)),
        None => true,
    }
}

pub fn override_url_allow_list(list: Option<Vec<String>>) {
    let mut guard = URL_ALLOW_LIST
        .write()
        .expect("url allow list lock poisoned");
    *guard = list;
}

pub fn reload_url_allow_list_from_env() {
    override_url_allow_list(read_allow_list_from_env());
}

fn enforce_payload_limit(
    body: &mut Vec<Value>,
    actions: &mut Vec<Value>,
    limit: usize,
    metrics: &mut RenderMetrics,
    warnings: &mut Vec<String>,
    warning: &str,
) {
    loop {
        let candidate = json!({
            "type": "AdaptiveCard",
            "$schema": ADAPTIVE_SCHEMA,
            "version": ADAPTIVE_VERSION,
            "body": body,
            "actions": actions,
        });
        if serde_json::to_vec(&candidate).map(|v| v.len()).unwrap_or(0) <= limit {
            break;
        }
        metrics.limit_exceeded = true;
        if !warnings.iter().any(|w| w == warning) {
            warnings.push(warning.into());
        }
        if !actions.is_empty() {
            actions.pop();
            continue;
        }
        if !body.is_empty() {
            body.pop();
            continue;
        }
        break;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messaging_card::ir::MessageCardIrBuilder;
    use serde_json::json;

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

    #[test]
    fn premium_link_includes_state_jwt() {
        let app_link = AppLink {
            base_url: "https://premium.example/deeplink".into(),
            secret: None,
            tenant: Some("acme".into()),
            scope: Some("beta".into()),
            state: Some(json!({"flow": "demo"})),
            jwt: Some(AppLinkJwt {
                secret: "jwt-secret".into(),
                algorithm: "HS256".into(),
                audience: Some("preview".into()),
                issuer: Some("gsm".into()),
                ttl_seconds: 300,
            }),
        };
        let link =
            build_signed_link(&app_link, "https://example.com/docs").expect("link must build");
        assert!(link.contains("state_jwt="));
    }

    #[test]
    fn normalize_state_rejects_scalars() {
        assert!(normalize_state_payload(&Value::String("oops".into())).is_none());
        assert!(normalize_state_payload(&Value::Bool(true)).is_none());
    }

    #[test]
    fn normalize_state_enforces_size_limit() {
        let large = "x".repeat(MAX_STATE_BYTES + 10);
        let value = json!({"blob": large});
        assert!(normalize_state_payload(&value).is_none());
    }
}
