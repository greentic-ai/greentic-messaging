use crate::{CardBlock, MessageCard, MessageEnvelope, OutKind, OutMessage};
use anyhow::{bail, Result};
use time::OffsetDateTime;

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
    use crate::{make_tenant_ctx, CardAction, Platform};
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
}
