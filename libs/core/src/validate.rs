use crate::{
    CardBlock, MessageCard, MessageEnvelope, OutKind, OutMessage, ProviderMessageEnvelope,
    ReplyInput, SendInput, SendMetadata,
};
use anyhow::{Result, bail};
use time::OffsetDateTime;

/// Describes a validation failure for provider-core inputs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationIssue {
    pub field: &'static str,
    pub message: String,
}

impl ValidationIssue {
    pub fn new(field: &'static str, message: impl Into<String>) -> Self {
        Self {
            field,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ValidationIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.field, self.message)
    }
}

impl std::error::Error for ValidationIssue {}

/// Convenience alias for validation results that do not use `anyhow`.
pub type ValidationResult<T> = std::result::Result<T, ValidationIssue>;

/// Validates the send provider-core input.
pub fn validate_send_input(input: &SendInput) -> ValidationResult<()> {
    if input.to.trim().is_empty() {
        return Err(ValidationIssue::new("to", "recipient is required"));
    }

    let has_text = input
        .text
        .as_ref()
        .map(|t| !t.trim().is_empty())
        .unwrap_or(false);
    let has_attachments = !input.attachments.is_empty();

    if !has_text && !has_attachments {
        return Err(ValidationIssue::new(
            "content",
            "provide text or at least one attachment",
        ));
    }

    if let Some(text) = &input.text
        && text.trim().is_empty()
    {
        return Err(ValidationIssue::new("text", "text cannot be empty"));
    }

    for (idx, att) in input.attachments.iter().enumerate() {
        if att.name.trim().is_empty() {
            return Err(ValidationIssue::new(
                "attachments",
                format!("attachment #{idx} name is empty"),
            ));
        }
        if att.content_type.trim().is_empty() {
            return Err(ValidationIssue::new(
                "attachments",
                format!("attachment #{idx} content_type is empty"),
            ));
        }
        if att.data_base64.trim().is_empty() {
            return Err(ValidationIssue::new(
                "attachments",
                format!("attachment #{idx} data_base64 is empty"),
            ));
        }
    }

    if let Some(meta) = &input.metadata {
        if let Some(thread_id) = &meta.thread_id
            && thread_id.trim().is_empty()
        {
            return Err(ValidationIssue::new(
                "metadata.thread_id",
                "thread_id cannot be empty",
            ));
        }
        if let Some(reply_to) = &meta.reply_to
            && reply_to.trim().is_empty()
        {
            return Err(ValidationIssue::new(
                "metadata.reply_to",
                "reply_to cannot be empty",
            ));
        }
        for (idx, tag) in meta.tags.iter().enumerate() {
            if tag.trim().is_empty() {
                return Err(ValidationIssue::new(
                    "metadata.tags",
                    format!("tag #{idx} is empty"),
                ));
            }
        }
    }

    Ok(())
}

/// Validates reply input, ensuring the reply target is present.
pub fn validate_reply_input(input: &ReplyInput) -> ValidationResult<()> {
    if input.reply_to.trim().is_empty() {
        return Err(ValidationIssue::new(
            "reply_to",
            "reply_to message identifier is required",
        ));
    }

    let send_like = SendInput {
        to: input.to.clone(),
        text: input.text.clone(),
        attachments: input.attachments.clone(),
        metadata: Some(SendMetadata {
            thread_id: input.metadata.as_ref().and_then(|m| m.thread_id.clone()),
            reply_to: Some(input.reply_to.clone()),
            tags: input
                .metadata
                .as_ref()
                .map(|m| m.tags.clone())
                .unwrap_or_default(),
        }),
    };

    validate_send_input(&send_like)
}

/// Normalizes a canonical message envelope (trims text and metadata).
pub fn normalize_envelope(env: &mut ProviderMessageEnvelope) {
    if let Some(text) = env.text.as_mut() {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            env.text = None;
        } else {
            *text = trimmed.to_string();
        }
    }

    if let Some(user) = env.user_id.as_mut() {
        let trimmed = user.trim();
        if trimmed.is_empty() {
            env.user_id = None;
        } else {
            *user = trimmed.to_string();
        }
    }

    let mut normalized = std::collections::BTreeMap::new();
    for (k, v) in env.metadata.iter() {
        let key = k.trim();
        let value = v.trim();
        if key.is_empty() || value.is_empty() {
            continue;
        }
        normalized.insert(key.to_string(), value.to_string());
    }
    env.metadata = normalized;
}

/// Validates an inbound [`MessageEnvelope`] for required fields and timestamp correctness.
///
/// ```
/// use gsm_core::{validate_envelope, MessageEnvelope, Platform};
/// use std::collections::BTreeMap;
///
/// let env = MessageEnvelope {
///     tenant: "acme".into(),
///     platform: Platform::Teams,
///     chat_id: "chat-1".into(),
///     user_id: "user-7".into(),
///     thread_id: None,
///     msg_id: "msg-99".into(),
///     text: Some("Hello".into()),
///     timestamp: "2024-01-01T00:00:00Z".into(),
///     context: BTreeMap::new(),
/// };
///
/// validate_envelope(&env).unwrap();
/// ```
pub fn validate_envelope(env: &MessageEnvelope) -> Result<()> {
    if env.tenant.trim().is_empty() {
        bail!("tenant empty");
    }
    if env.chat_id.trim().is_empty() {
        bail!("chat_id empty");
    }
    if env.user_id.trim().is_empty() {
        bail!("user_id empty");
    }
    if env.msg_id.trim().is_empty() {
        bail!("msg_id empty");
    }
    // timestamp is ISO-8601-ish; accept RFC3339
    OffsetDateTime::parse(
        &env.timestamp,
        &time::format_description::well_known::Rfc3339,
    )
    .map_err(|e| anyhow::anyhow!("invalid timestamp: {e}"))?;
    Ok(())
}

/// Validates an outbound [`OutMessage`] before it is sent to translators.
///
/// ```
/// use gsm_core::{make_tenant_ctx, validate_out, OutKind, OutMessage, Platform};
///
/// let out = OutMessage {
///     ctx: make_tenant_ctx("acme".into(), None, None),
///     tenant: "acme".into(),
///     platform: Platform::Telegram,
///     chat_id: "chat-1".into(),
///     thread_id: None,
///     kind: OutKind::Text,
///     text: Some("Hello".into()),
///     message_card: None,
///     #[cfg(feature = "adaptive-cards")]
///     adaptive_card: None,
///     meta: Default::default(),
/// };
///
/// validate_out(&out).unwrap();
/// ```
pub fn validate_out(out: &OutMessage) -> Result<()> {
    if out.tenant.trim().is_empty() {
        bail!("tenant empty");
    }
    if out.chat_id.trim().is_empty() {
        bail!("chat_id empty");
    }
    match out.kind {
        OutKind::Text => {
            if out.text.as_deref().unwrap_or("").trim().is_empty() {
                bail!("text empty");
            }
        }
        OutKind::Card => {
            let card = out
                .message_card
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("card missing"))?;
            validate_card(card)?;
        }
    }
    Ok(())
}

/// Validates the structure and content of a [`MessageCard`].
///
/// ```
/// use gsm_core::{validate_card, CardAction, CardBlock, MessageCard};
/// use serde_json::json;
///
/// let card = MessageCard {
///     title: Some("Weather".into()),
///     body: vec![
///         CardBlock::Text { text: "Forecast".into(), markdown: false },
///         CardBlock::Fact { label: "High".into(), value: "22C".into() },
///     ],
///     actions: vec![
///         CardAction::OpenUrl {
///             title: "Details".into(),
///             url: "https://example.com".into(),
///             jwt: false,
///         },
///         CardAction::Postback { title: "Ack".into(), data: json!({"ok": true}) },
///     ],
/// };
///
/// validate_card(&card).unwrap();
/// ```
pub fn validate_card(card: &MessageCard) -> Result<()> {
    if card.body.is_empty() && card.title.as_deref().unwrap_or("").is_empty() {
        bail!("card must have title or body");
    }
    for block in &card.body {
        match block {
            CardBlock::Text { text, .. } if text.trim().is_empty() => bail!("empty text block"),
            CardBlock::Fact { label, value }
                if label.trim().is_empty() || value.trim().is_empty() =>
            {
                bail!("empty fact")
            }
            CardBlock::Image { url } if url.trim().is_empty() => bail!("empty image url"),
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CardAction, Platform, make_tenant_ctx};
    use serde_json::json;
    use std::collections::BTreeMap;

    fn sample_envelope() -> MessageEnvelope {
        MessageEnvelope {
            tenant: "acme".into(),
            platform: Platform::Teams,
            chat_id: "chat-1".into(),
            user_id: "user-7".into(),
            thread_id: None,
            msg_id: "msg-1".into(),
            text: Some("Hello".into()),
            timestamp: "2024-01-01T00:00:00Z".into(),
            context: BTreeMap::new(),
        }
    }

    fn sample_out(kind: OutKind) -> OutMessage {
        OutMessage {
            ctx: make_tenant_ctx("acme".into(), None, None),
            tenant: "acme".into(),
            platform: Platform::Telegram,
            chat_id: "chat-1".into(),
            thread_id: None,
            kind,
            text: Some("Hello".into()),
            message_card: None,
            #[cfg(feature = "adaptive-cards")]
            adaptive_card: None,
            meta: Default::default(),
        }
    }

    #[test]
    fn envelope_rejects_empty_tenant() {
        let mut env = sample_envelope();
        env.tenant = "   ".into();
        assert!(validate_envelope(&env).is_err());
    }

    #[test]
    fn envelope_rejects_bad_timestamp() {
        let mut env = sample_envelope();
        env.timestamp = "not-a-date".into();
        assert!(validate_envelope(&env).is_err());
    }

    #[test]
    fn out_text_requires_content() {
        let mut out = sample_out(OutKind::Text);
        out.text = Some("   ".into());
        assert!(validate_out(&out).is_err());
    }

    #[test]
    fn out_card_requires_message_card() {
        let out = sample_out(OutKind::Card);
        assert!(validate_out(&out).is_err());
    }

    #[test]
    fn card_requires_body_or_title() {
        let card = MessageCard {
            title: None,
            body: vec![],
            actions: vec![],
        };
        assert!(validate_card(&card).is_err());
    }

    #[test]
    fn card_rejects_empty_fact_fields() {
        let card = MessageCard {
            title: Some("Facts".into()),
            body: vec![CardBlock::Fact {
                label: " ".into(),
                value: "".into(),
            }],
            actions: vec![],
        };
        assert!(validate_card(&card).is_err());
    }

    #[test]
    fn card_accepts_valid_structure() {
        let card = MessageCard {
            title: Some("Weather".into()),
            body: vec![
                CardBlock::Text {
                    text: "Sunny".into(),
                    markdown: false,
                },
                CardBlock::Fact {
                    label: "High".into(),
                    value: "22C".into(),
                },
            ],
            actions: vec![CardAction::Postback {
                title: "Ack".into(),
                data: json!({"ok": true}),
            }],
        };
        assert!(validate_card(&card).is_ok());
    }

    #[test]
    fn send_validation_requires_text_or_attachment() {
        let input = SendInput {
            to: "channel-123".into(),
            text: None,
            attachments: vec![],
            metadata: None,
        };
        assert!(validate_send_input(&input).is_err());
    }

    #[test]
    fn reply_validation_requires_reply_to() {
        let input = ReplyInput {
            to: "channel-1".into(),
            reply_to: "   ".into(),
            text: Some("hi".into()),
            attachments: vec![],
            metadata: None,
        };
        assert!(validate_reply_input(&input).is_err());
    }

    #[test]
    fn normalize_envelope_trims_and_prunes() {
        let mut env = ProviderMessageEnvelope {
            id: "id-1".into(),
            tenant: make_tenant_ctx("acme".into(), None, None),
            channel: "channel".into(),
            session_id: "session".into(),
            user_id: Some("  ".into()),
            text: Some(" hi  ".into()),
            attachments: vec![],
            metadata: BTreeMap::from_iter([(" key ".into(), " value ".into())]),
        };
        normalize_envelope(&mut env);
        assert_eq!(env.text.as_deref(), Some("hi"));
        assert!(env.user_id.is_none());
        assert!(!env.metadata.contains_key(" key "));
        assert_eq!(env.metadata.get("key"), Some(&"value".to_string()));
    }
}
