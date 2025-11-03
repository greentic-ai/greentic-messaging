//! Helpers for translating platform-agnostic messages into provider specific payloads.
//!
//! The main entry point is the [`Translator`] trait, which is implemented for each supported
//! outbound channel. Translators accept a [`gsm_core::OutMessage`] and emit one or more platform
//! payloads ready to be dispatched.

use anyhow::{Result, anyhow};
use gsm_core::{CardAction, CardBlock, MessageCard, OutKind, OutMessage};
use security::{
    hash::state_hash_out,
    jwt::{ActionClaims, JwtSigner},
    links::{action_base_url, action_ttl, build_action_url},
};
use serde_json::{Value, json};
use time::Duration;
use unicode_segmentation::UnicodeSegmentation;
use uuid::Uuid;

use crate::telemetry::translate_with_span;

/// Converts a platform-agnostic [`OutMessage`](gsm_core::OutMessage) into a list of platform specific payloads.
///
/// Implementations should never mutate the original message and must return an error when required
/// fields are missing for the requested conversion.
pub trait Translator {
    fn to_platform(&self, out: &OutMessage) -> Result<Vec<Value>>;
}

pub fn secure_action_url(out: &OutMessage, title: &str, url: &str) -> String {
    if let Some(config) = load_action_config() {
        let scope = format!("{}.{}", out.platform.as_str(), slugify(title));
        let claims = ActionClaims::new(
            out.chat_id.clone(),
            out.tenant.clone(),
            scope,
            state_hash_out(out),
            Some(url.to_string()),
            config.ttl,
        );
        if let Ok(link) = build_action_url(&config.base, claims, &config.signer) {
            return link;
        }
    }
    url.to_string()
}

struct ActionLinkConfig {
    base: String,
    signer: JwtSigner,
    ttl: Duration,
}

fn load_action_config() -> Option<ActionLinkConfig> {
    let base = action_base_url()?;
    let signer = JwtSigner::from_env().ok()?;
    Some(ActionLinkConfig {
        base,
        signer,
        ttl: action_ttl(),
    })
}

fn slugify(input: &str) -> String {
    let mut slug = String::new();
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
        } else if (ch.is_whitespace() || ch == '-' || ch == '_') && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let trimmed = slug.trim_matches('-');
    if trimmed.is_empty() {
        format!("open-{}", Uuid::new_v4().simple())
    } else {
        trimmed.to_string()
    }
}

pub mod slack;
pub mod teams;
mod telemetry;
pub mod webex;

/// Translator that produces Telegram specific API requests.
///
/// ```
/// use gsm_translator::{TelegramTranslator, Translator};
/// use gsm_core::{make_tenant_ctx, OutMessage, OutKind, Platform};
/// use serde_json::json;
///
/// let mut message = OutMessage {
///     ctx: make_tenant_ctx("acme".into(), None, None),
///     tenant: "acme".into(),
///     platform: Platform::Telegram,
///     chat_id: "chat-1".into(),
///     thread_id: None,
///     kind: OutKind::Text,
///     text: Some("Hello".into()),
///     message_card: None,
///     meta: Default::default(),
/// };
/// let translator = TelegramTranslator::new();
/// let payloads = translator.to_platform(&message).unwrap();
/// assert_eq!(payloads, vec![json!({
///   "method": "sendMessage",
///   "parse_mode": "HTML",
///   "text": "Hello"
/// })]);
/// ```
pub struct TelegramTranslator;

impl TelegramTranslator {
    /// Creates a new instance of the Telegram translator.
    pub fn new() -> Self {
        Self
    }

    fn render_text(text: &str) -> Value {
        json!({
          "method": "sendMessage",
          "parse_mode": "HTML",
          "text": html_escape(text),
        })
    }

    fn render_card(out: &OutMessage, card: &MessageCard) -> Vec<Value> {
        let mut parts: Vec<String> = Vec::new();
        if let Some(t) = &card.title {
            parts.push(format!("<b>{}</b>", html_escape(t)));
        }
        for block in &card.body {
            match block {
                CardBlock::Text { text, .. } => parts.push(html_escape(text)),
                CardBlock::Fact { label, value } => parts.push(format!(
                    "• <b>{}</b>: {}",
                    html_escape(label),
                    html_escape(value)
                )),
                CardBlock::Image { url } => parts.push(url.clone()),
            }
        }

        let mut payloads = vec![json!({
          "method": "sendMessage",
          "parse_mode": "HTML",
          "text": parts.join("\n"),
        })];

        if !card.actions.is_empty() {
            let mut keyboard: Vec<Vec<Value>> = Vec::new();
            for action in &card.actions {
                match action {
                    CardAction::OpenUrl { title, url, .. } => {
                        let href = secure_action_url(out, title, url);
                        keyboard.push(vec![json!({ "text": title, "url": href })]);
                    }
                    CardAction::Postback { title, data } => {
                        let data_str = serde_json::to_string(data).unwrap_or_else(|_| "{}".into());
                        keyboard.push(vec![json!({ "text": title, "callback_data": data_str })]);
                    }
                }
            }
            payloads.push(json!({
              "method": "sendMessage",
              "parse_mode": "HTML",
              "text": "Actions:",
              "reply_markup": { "inline_keyboard": keyboard },
            }));
        }

        payloads
    }
}

impl Default for TelegramTranslator {
    fn default() -> Self {
        Self::new()
    }
}

impl Translator for TelegramTranslator {
    fn to_platform(&self, out: &OutMessage) -> Result<Vec<Value>> {
        translate_with_span(out, "telegram", || match out.kind {
            OutKind::Text => {
                let text = out.text.as_deref().ok_or_else(|| anyhow!("missing text"))?;
                Ok(vec![Self::render_text(text)])
            }
            OutKind::Card => {
                let card = out
                    .message_card
                    .as_ref()
                    .ok_or_else(|| anyhow!("missing card"))?;
                Ok(Self::render_card(out, card))
            }
        })
    }
}

fn html_escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for grapheme in UnicodeSegmentation::graphemes(text, true) {
        escaped.push_str(match grapheme {
            "&" => "&amp;",
            "<" => "&lt;",
            ">" => "&gt;",
            _ => grapheme,
        });
    }
    escaped
}

pub struct WebChatTranslator;

impl WebChatTranslator {
    /// Creates a new instance of the WebChat translator.
    pub fn new() -> Self {
        Self
    }
}

impl Default for WebChatTranslator {
    fn default() -> Self {
        Self::new()
    }
}

/// Translator that turns messages into WebChat payloads.
///
/// ```
/// use gsm_translator::{WebChatTranslator, Translator};
/// use gsm_core::{make_tenant_ctx, OutMessage, OutKind, Platform};
/// use serde_json::json;
///
/// let mut message = OutMessage {
///     ctx: make_tenant_ctx("acme".into(), None, None),
///     tenant: "acme".into(),
///     platform: Platform::WebChat,
///     chat_id: "thread-42".into(),
///     thread_id: None,
///     kind: OutKind::Text,
///     text: Some("Hello WebChat".into()),
///     message_card: None,
///     meta: Default::default(),
/// };
///
/// let translator = WebChatTranslator::new();
/// let payloads = translator.to_platform(&message).unwrap();
/// assert_eq!(payloads, vec![json!({
///   "kind": "text",
///   "text": "Hello WebChat"
/// })]);
/// ```
impl Translator for WebChatTranslator {
    fn to_platform(&self, out: &OutMessage) -> Result<Vec<Value>> {
        translate_with_span(out, "webchat", || {
            let payload = match out.kind {
                OutKind::Text => json!({
                  "kind": "text",
                  "text": out.text.clone().unwrap_or_default(),
                }),
                OutKind::Card => {
                    let mut card = out
                        .message_card
                        .clone()
                        .ok_or_else(|| anyhow!("missing card"))?;
                    for action in card.actions.iter_mut() {
                        if let CardAction::OpenUrl { title, url, .. } = action {
                            let signed = secure_action_url(out, title, url);
                            *url = signed;
                        }
                    }
                    json!({
                      "kind": "card",
                      "card": card,
                    })
                }
            };
            Ok(vec![payload])
        })
    }
}

/// Translator for Webex messages.
pub struct WebexTranslator;

impl WebexTranslator {
    pub fn new() -> Self {
        Self
    }
}

impl Default for WebexTranslator {
    fn default() -> Self {
        Self::new()
    }
}

impl Translator for WebexTranslator {
    fn to_platform(&self, out: &OutMessage) -> Result<Vec<Value>> {
        let payload = crate::webex::to_webex_payload(out)?;
        Ok(vec![payload])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::teams::to_teams_adaptive;
    use gsm_core::{
        CardAction, CardBlock, MessageCard, OutKind, OutMessage, Platform, make_tenant_ctx,
    };
    use security::jwt::JwtSigner;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn sample_out_message(kind: OutKind) -> OutMessage {
        OutMessage {
            ctx: make_tenant_ctx("acme".into(), None, None),
            tenant: "acme".into(),
            platform: Platform::Telegram,
            chat_id: "chat-1".into(),
            thread_id: None,
            kind,
            text: None,
            message_card: None,
            meta: Default::default(),
        }
    }

    #[test]
    fn telegram_text_payload() {
        let mut out = sample_out_message(OutKind::Text);
        out.text = Some("Hello & <world>".into());

        let translator = TelegramTranslator::new();
        let payloads = translator.to_platform(&out).unwrap();

        assert_eq!(
            payloads,
            vec![json!({
              "method": "sendMessage",
              "parse_mode": "HTML",
              "text": "Hello &amp; &lt;world&gt;"
            })]
        );
    }

    #[test]
    fn telegram_card_payloads() {
        let mut out = sample_out_message(OutKind::Card);
        let _guard = env_lock().lock().unwrap();
        unsafe {
            std::env::remove_var("ACTION_BASE_URL");
        }
        unsafe {
            std::env::remove_var("JWT_SECRET");
        }
        unsafe {
            std::env::remove_var("JWT_ALG");
        }
        out.message_card = Some(MessageCard {
            title: Some("Weather".into()),
            body: vec![
                CardBlock::Text {
                    text: "Line 1".into(),
                    markdown: false,
                },
                CardBlock::Fact {
                    label: "High".into(),
                    value: "20C".into(),
                },
            ],
            actions: vec![
                CardAction::OpenUrl {
                    title: "View".into(),
                    url: "https://example.com".into(),
                    jwt: false,
                },
                CardAction::Postback {
                    title: "Ack".into(),
                    data: json!({"ok": true}),
                },
            ],
        });

        let translator = TelegramTranslator::new();
        let payloads = translator.to_platform(&out).unwrap();

        assert_eq!(payloads.len(), 2);
        assert_eq!(
            payloads[0],
            json!({
              "method": "sendMessage",
              "parse_mode": "HTML",
              "text": "<b>Weather</b>\nLine 1\n• <b>High</b>: 20C"
            })
        );
        assert_eq!(
            payloads[1],
            json!({
              "method": "sendMessage",
              "parse_mode": "HTML",
              "text": "Actions:",
              "reply_markup": {
                "inline_keyboard": [
                  [{"text": "View", "url": "https://example.com"}],
                  [{"text": "Ack", "callback_data": "{\"ok\":true}"}]
                ]
              }
            })
        );
    }

    #[test]
    fn telegram_card_actions_are_signed_when_configured() {
        let _guard = env_lock().lock().unwrap();
        unsafe {
            std::env::set_var("ACTION_BASE_URL", "https://actions.test/a");
        }
        unsafe {
            std::env::set_var("JWT_ALG", "HS256");
        }
        unsafe {
            std::env::set_var("JWT_SECRET", "signing-secret");
        }
        let mut out = sample_out_message(OutKind::Card);
        out.message_card = Some(MessageCard {
            title: None,
            body: vec![],
            actions: vec![CardAction::OpenUrl {
                title: "Open".into(),
                url: "https://example.com/path".into(),
                jwt: true,
            }],
        });

        let translator = TelegramTranslator::new();
        let payloads = translator.to_platform(&out).unwrap();

        assert_eq!(payloads.len(), 2);
        let keyboard = &payloads[1]["reply_markup"]["inline_keyboard"];
        let signed_url = keyboard[0][0]["url"].as_str().unwrap();
        assert!(signed_url.starts_with("https://actions.test/a?action="));

        let token = signed_url.split("action=").nth(1).expect("token missing");
        let decoded_token = urlencoding::decode(token).expect("decode token");
        let signer = JwtSigner::from_env().expect("verify signer");
        let claims = signer.verify(&decoded_token).expect("claims");
        assert_eq!(claims.redirect.as_deref(), Some("https://example.com/path"));
        assert_eq!(claims.tenant, out.tenant);

        unsafe {
            std::env::remove_var("ACTION_BASE_URL");
        }
        unsafe {
            std::env::remove_var("JWT_SECRET");
        }
        unsafe {
            std::env::remove_var("JWT_ALG");
        }
    }

    #[test]
    fn webchat_text_payload() {
        let mut out = sample_out_message(OutKind::Text);
        out.platform = Platform::WebChat;
        out.text = Some("Hello WebChat".into());

        let translator = WebChatTranslator::new();
        let payloads = translator.to_platform(&out).unwrap();

        assert_eq!(
            payloads,
            vec![json!({
              "kind": "text",
              "text": "Hello WebChat"
            })]
        );
    }

    #[test]
    fn webchat_card_payload() {
        let mut out = sample_out_message(OutKind::Card);
        out.message_card = Some(MessageCard {
            title: Some("Title".into()),
            body: vec![CardBlock::Text {
                text: "Hello".into(),
                markdown: true,
            }],
            actions: vec![],
        });

        out.platform = Platform::WebChat;
        let expected_card = out.message_card.clone();

        let translator = WebChatTranslator::new();
        let payloads = translator.to_platform(&out).unwrap();

        assert_eq!(
            payloads,
            vec![json!({
              "kind": "card",
              "card": expected_card
            })]
        );
    }

    #[test]
    fn teams_card_payload() {
        let _guard = env_lock().lock().unwrap();
        unsafe {
            std::env::set_var("ACTION_BASE_URL", "https://actions.test/a");
        }
        unsafe {
            std::env::set_var("JWT_ALG", "HS256");
        }
        unsafe {
            std::env::set_var("JWT_SECRET", "signing-secret");
        }
        let card = MessageCard {
            title: Some("Weather".into()),
            body: vec![
                CardBlock::Text {
                    text: "Line".into(),
                    markdown: false,
                },
                CardBlock::Fact {
                    label: "High".into(),
                    value: "20C".into(),
                },
            ],
            actions: vec![CardAction::OpenUrl {
                title: "View".into(),
                url: "https://example.com".into(),
                jwt: false,
            }],
        };

        let mut out = sample_out_message(OutKind::Card);
        out.platform = Platform::Teams;
        let adaptive = to_teams_adaptive(&card, &out).unwrap();
        assert_eq!(adaptive["type"], "AdaptiveCard");
        assert_eq!(adaptive["body"][0]["text"], "Weather");
        assert_eq!(adaptive["actions"][0]["type"], "Action.OpenUrl");
        let action_url = adaptive["actions"][0]["url"].as_str().unwrap();
        assert!(action_url.starts_with("https://actions.test/a?action="));

        unsafe {
            std::env::remove_var("ACTION_BASE_URL");
        }
        unsafe {
            std::env::remove_var("JWT_SECRET");
        }
        unsafe {
            std::env::remove_var("JWT_ALG");
        }
    }
}
