//! Helpers for translating platform-agnostic messages into provider specific payloads.
//!
//! The main entry point is the [`Translator`] trait, which is implemented for each supported
//! outbound channel. Translators accept a [`gsm_core::OutMessage`] and emit one or more platform
//! payloads ready to be dispatched.

use anyhow::{anyhow, Result};
use gsm_core::{CardAction, CardBlock, MessageCard, OutKind, OutMessage};
use serde_json::{json, Value};
use unicode_segmentation::UnicodeSegmentation;

/// Converts a platform-agnostic [`OutMessage`](gsm_core::OutMessage) into a list of platform specific payloads.
///
/// Implementations should never mutate the original message and must return an error when required
/// fields are missing for the requested conversion.
pub trait Translator {
    fn to_platform(&self, out: &OutMessage) -> Result<Vec<Value>>;
}

/// Optionally appends a signed token to an URL when JWT signing is enabled.
///
/// ```
/// use gsm_translator::sign_url_if_needed;
///
/// let unsigned = sign_url_if_needed("https://example.com/resource", false);
/// assert_eq!(unsigned, "https://example.com/resource");
/// ```
pub fn sign_url_if_needed(url: &str, jwt: bool) -> String {
    if !jwt {
        return url.to_string();
    }
    let key = std::env::var("LINK_JWT_SECRET").unwrap_or_else(|_| "dev-secret".into());
    let exp =
        (time::OffsetDateTime::now_utc() + time::Duration::hours(1)).unix_timestamp() as usize;
    let claims = json!({ "sub": url, "exp": exp });
    let token = jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(key.as_bytes()),
    )
    .unwrap_or_default();
    let sep = if url.contains('?') { '&' } else { '?' };
    format!("{url}{sep}t={token}")
}

pub mod slack;
pub mod teams;

/// Translator that produces Telegram specific API requests.
///
/// ```
/// use gsm_translator::{TelegramTranslator, Translator};
/// use gsm_core::{OutMessage, OutKind, Platform};
/// use serde_json::json;
///
/// let mut message = OutMessage {
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

    fn render_card(card: &MessageCard) -> Vec<Value> {
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
                    CardAction::OpenUrl { title, url, jwt } => {
                        let href = sign_url_if_needed(url, *jwt);
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

impl Translator for TelegramTranslator {
    fn to_platform(&self, out: &OutMessage) -> Result<Vec<Value>> {
        match out.kind {
            OutKind::Text => {
                let text = out.text.as_deref().ok_or_else(|| anyhow!("missing text"))?;
                Ok(vec![Self::render_text(text)])
            }
            OutKind::Card => {
                let card = out
                    .message_card
                    .as_ref()
                    .ok_or_else(|| anyhow!("missing card"))?;
                Ok(Self::render_card(card))
            }
        }
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

/// Translator that turns messages into WebChat payloads.
///
/// ```
/// use gsm_translator::{WebChatTranslator, Translator};
/// use gsm_core::{OutMessage, OutKind, Platform};
/// use serde_json::json;
///
/// let mut message = OutMessage {
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
        let payload = match out.kind {
            OutKind::Text => json!({
              "kind": "text",
              "text": out.text.clone().unwrap_or_default(),
            }),
            OutKind::Card => json!({
              "kind": "card",
              "card": out.message_card,
            }),
        };
        Ok(vec![payload])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::teams::to_teams_adaptive;
    use gsm_core::{CardAction, CardBlock, MessageCard, OutKind, OutMessage, Platform};
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn sample_out_message(kind: OutKind) -> OutMessage {
        OutMessage {
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
    fn telegram_card_jwt_links_are_signed() {
        let _guard = env_lock().lock().unwrap();
        std::env::set_var("LINK_JWT_SECRET", "signing-secret");
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
        assert!(keyboard[0][0]["url"]
            .as_str()
            .unwrap()
            .starts_with("https://example.com/path?t="));

        let signed_url = keyboard[0][0]["url"].as_str().unwrap();
        let token = signed_url
            .split("t=")
            .nth(1)
            .and_then(|rest| rest.split('&').next())
            .expect("token missing");

        let decoded = jsonwebtoken::decode::<serde_json::Value>(
            token,
            &jsonwebtoken::DecodingKey::from_secret(b"signing-secret"),
            &jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::HS256),
        )
        .unwrap();
        assert_eq!(
            decoded.claims["sub"],
            serde_json::Value::String("https://example.com/path".into())
        );
        std::env::remove_var("LINK_JWT_SECRET");
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
    fn sign_url_if_needed_appends_token() {
        let _guard = env_lock().lock().unwrap();
        std::env::set_var("LINK_JWT_SECRET", "another-secret");
        let signed = sign_url_if_needed("https://example.com?q=1", true);
        assert!(signed.starts_with("https://example.com?q=1"));
        assert!(signed.contains("t="));
        let token = signed
            .split("t=")
            .nth(1)
            .and_then(|rest| rest.split('&').next())
            .expect("token missing");

        let decoded = jsonwebtoken::decode::<serde_json::Value>(
            token,
            &jsonwebtoken::DecodingKey::from_secret(b"another-secret"),
            &jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::HS256),
        )
        .unwrap();
        assert_eq!(
            decoded.claims["sub"],
            serde_json::Value::String("https://example.com?q=1".into())
        );
        std::env::remove_var("LINK_JWT_SECRET");
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

        let adaptive = to_teams_adaptive(&card).unwrap();
        assert_eq!(adaptive["type"], "AdaptiveCard");
        assert_eq!(adaptive["body"][0]["text"], "Weather");
        assert_eq!(adaptive["actions"][0]["type"], "Action.OpenUrl");
    }
}
