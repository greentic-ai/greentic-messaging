use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Supported messaging platforms (kept small and stable).
///
/// ```
/// use gsm_core::Platform;
///
/// let p = Platform::Telegram;
/// assert_eq!(p.as_str(), "telegram");
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Teams,
    Slack,
    Telegram,
    WhatsApp,
    WebChat,
}

impl Platform {
    /// Returns the lowercase string identifier used in NATS subjects and payloads.
    pub fn as_str(&self) -> &'static str {
        match self {
            Platform::Teams => "teams",
            Platform::Slack => "slack",
            Platform::Telegram => "telegram",
            Platform::WhatsApp => "whatsapp",
            Platform::WebChat => "webchat",
        }
    }
}

/// Normalized inbound message from webhooks.
///
/// ```
/// use gsm_core::{MessageEnvelope, Platform};
/// use std::collections::BTreeMap;
///
/// let mut env = MessageEnvelope {
///     tenant: "acme".into(),
///     platform: Platform::Slack,
///     chat_id: "room-42".into(),
///     user_id: "user-1".into(),
///     thread_id: None,
///     msg_id: "msg-99".into(),
///     text: Some("hello team".into()),
///     timestamp: "2024-01-01T00:00:00Z".into(),
///     context: BTreeMap::new(),
/// };
/// env.context.insert("ip".into(), serde_json::json!("127.0.0.1"));
/// assert_eq!(env.platform.as_str(), "slack");
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MessageEnvelope {
    pub tenant: String,
    pub platform: Platform,
    pub chat_id: String,
    pub user_id: String,
    pub thread_id: Option<String>,
    pub msg_id: String, // idempotency key (platform event/message id)
    pub text: Option<String>,
    pub timestamp: String, // ISO-8601
    #[serde(default)]
    pub context: BTreeMap<String, serde_json::Value>, // headers/extra/raw pointers
}

/// What runner emits for egress workers.
///
/// ```
/// use gsm_core::{OutKind, OutMessage, Platform};
///
/// let out = OutMessage {
///     tenant: "acme".into(),
///     platform: Platform::Telegram,
///     chat_id: "chat-1".into(),
///     thread_id: None,
///     kind: OutKind::Text,
///     text: Some("Hello".into()),
///     message_card: None,
///     meta: Default::default(),
/// };
///
/// assert_eq!(out.kind, OutKind::Text);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OutMessage {
    pub tenant: String,
    pub platform: Platform, // usually same as incoming
    pub chat_id: String,
    pub thread_id: Option<String>,
    pub kind: OutKind,
    pub text: Option<String>,
    pub message_card: Option<MessageCard>,
    #[serde(default)]
    pub meta: BTreeMap<String, serde_json::Value>,
}

/// Output payload kinds supported by translators.
///
/// ```
/// use gsm_core::OutKind;
/// assert_eq!(format!("{:?}", OutKind::Text), "Text");
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum OutKind {
    Text,
    Card,
}

/// Minimal, canonical MessageCard v1.
///
/// ```
/// use gsm_core::{CardAction, CardBlock, MessageCard};
/// use serde_json::json;
///
/// let card = MessageCard {
///     title: Some("Weather".into()),
///     body: vec![
///         CardBlock::Text { text: "Sunny".into(), markdown: false },
///         CardBlock::Fact { label: "High".into(), value: "22C".into() },
///     ],
///     actions: vec![
///         CardAction::OpenUrl { title: "Detail".into(), url: "https://example.com".into(), jwt: false },
///         CardAction::Postback { title: "Ack".into(), data: json!({"ok": true}) },
///     ],
/// };
/// assert_eq!(card.body.len(), 2);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MessageCard {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default)]
    pub body: Vec<CardBlock>,
    #[serde(default)]
    pub actions: Vec<CardAction>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum CardBlock {
    /// Rich text block, optionally marked as Markdown.
    #[serde(rename = "text")]
    Text {
        text: String,
        #[serde(default)]
        markdown: bool,
    },
    /// Label/value pair shown as a fact row.
    #[serde(rename = "fact")]
    Fact { label: String, value: String },
    /// Image block referenced by URL.
    #[serde(rename = "image")]
    Image { url: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum CardAction {
    /// Opens an URL when invoked (optionally signed with JWT).
    #[serde(rename = "openUrl")]
    OpenUrl {
        title: String,
        url: String,
        #[serde(default)]
        jwt: bool,
    },
    /// Posts structured data back to the application.
    #[serde(rename = "postback")]
    Postback {
        title: String,
        data: serde_json::Value,
    },
}
