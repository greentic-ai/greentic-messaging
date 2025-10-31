use crate::context::make_tenant_ctx;
use crate::prelude::TenantCtx;
use greentic_types::{InvocationEnvelope, NodeError, NodeResult};
use serde::{Deserialize, Serialize};
use serde_json::{self, Value};
use std::collections::BTreeMap;

/// Supported messaging platforms (kept small and stable).
///
/// ```
/// use gsm_core::Platform;
///
/// let p = Platform::Telegram;
/// assert_eq!(p.as_str(), "telegram");
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Teams,
    Slack,
    Telegram,
    WhatsApp,
    WebChat,
    Webex,
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
            Platform::Webex => "webex",
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
    pub context: BTreeMap<String, Value>, // headers/extra/raw pointers
}

impl MessageEnvelope {
    /// Converts the message envelope into the canonical invocation envelope.
    pub fn into_invocation(self) -> NodeResult<InvocationEnvelope> {
        let ctx = make_tenant_ctx(self.tenant.clone(), None, Some(self.user_id.clone()));
        let payload = serde_json::to_vec(&self).map_err(|err| {
            NodeError::new(
                "SER_MESSAGE_ENVELOPE",
                "failed to serialize message envelope payload",
            )
            .with_source(err)
        })?;
        let metadata = if self.context.is_empty() {
            Vec::new()
        } else {
            serde_json::to_vec(&self.context).map_err(|err| {
                NodeError::new(
                    "SER_MESSAGE_METADATA",
                    "failed to serialize message envelope metadata",
                )
                .with_source(err)
            })?
        };
        Ok(InvocationEnvelope {
            ctx,
            flow_id: self.platform.as_str().to_string(),
            node_id: None,
            op: "on_message".to_string(),
            payload,
            metadata,
        })
    }
}

impl TryFrom<InvocationEnvelope> for MessageEnvelope {
    type Error = NodeError;

    fn try_from(inv: InvocationEnvelope) -> Result<Self, Self::Error> {
        let InvocationEnvelope {
            ctx,
            flow_id: _,
            node_id: _,
            op: _,
            payload,
            metadata,
        } = inv;

        let mut env: MessageEnvelope = serde_json::from_slice(&payload).map_err(|err| {
            NodeError::new(
                "DESER_ENVELOPE",
                "failed to deserialize invocation payload into MessageEnvelope",
            )
            .with_source(err)
        })?;

        if let Some(user) = ctx.user.clone().or_else(|| ctx.user_id.clone()) {
            env.user_id = String::from(user);
        }
        env.tenant = ctx.tenant.as_str().to_string();

        if !metadata.is_empty() {
            let context: BTreeMap<String, Value> =
                serde_json::from_slice(&metadata).map_err(|err| {
                    NodeError::new(
                        "DESER_METADATA",
                        "failed to deserialize invocation metadata into context",
                    )
                    .with_source(err)
                })?;
            env.context = context;
        }

        Ok(env)
    }
}

/// What runner emits for egress workers.
///
/// ```
/// use gsm_core::{OutKind, OutMessage, Platform};
///
/// let ctx = gsm_core::make_tenant_ctx("acme".into(), None, None);
/// let out = OutMessage {
///     ctx,
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutMessage {
    pub ctx: TenantCtx,
    pub tenant: String,
    pub platform: Platform, // usually same as incoming
    pub chat_id: String,
    pub thread_id: Option<String>,
    pub kind: OutKind,
    pub text: Option<String>,
    pub message_card: Option<MessageCard>,
    #[serde(default)]
    pub meta: BTreeMap<String, Value>,
}

impl OutMessage {
    /// Returns a stable identifier for tracing/logging, falling back to chat scope.
    pub fn message_id(&self) -> String {
        self.meta
            .get("source_msg_id")
            .and_then(|v| v.as_str())
            .or_else(|| self.meta.get("msg_id").and_then(|v| v.as_str()))
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("{}:{}", self.platform.as_str(), self.chat_id))
    }
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
    Postback { title: String, data: Value },
}
