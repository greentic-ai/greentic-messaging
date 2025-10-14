//! NATS subject helpers (multi-tenant/topic-safe)

use std::borrow::Cow;

/// Normalizes identifiers to be subject-safe (replace spaces, trim).
fn norm<S: AsRef<str>>(s: S) -> Cow<'static, str> {
    let mut t = s
        .as_ref()
        .trim()
        .replace([' ', '\t', '\n', '\r', '*', '>', '/'], "-");
    if t.is_empty() {
        t = "unknown".into();
    }
    Cow::Owned(t)
}

/// Inbound messages from ingress webhooks (normalized envelopes).
///
/// ```
/// use gsm_core::in_subject;
///
/// assert_eq!(
///     in_subject("acme", "teams", "room/42"),
///     "greentic.msg.in.acme.teams.room-42"
/// );
/// ```
pub fn in_subject(tenant: &str, platform: &str, chat_id: &str) -> String {
    format!(
        "greentic.msg.in.{}.{}.{}",
        norm(tenant),
        norm(platform),
        norm(chat_id)
    )
}

/// Outbound messages from runner to egress adapters.
///
/// ```
/// use gsm_core::out_subject;
///
/// assert_eq!(
///     out_subject("acme", "telegram", "chat 1"),
///     "greentic.msg.out.acme.telegram.chat-1"
/// );
/// ```
pub fn out_subject(tenant: &str, platform: &str, chat_id: &str) -> String {
    format!(
        "greentic.msg.out.{}.{}.{}",
        norm(tenant),
        norm(platform),
        norm(chat_id)
    )
}

/// Dead-letter queue subjects (direction = `"in"` or `"out"`).
///
/// ```
/// use gsm_core::dlq_subject;
///
/// assert_eq!(
///     dlq_subject("unknown", "acme", "slack"),
///     "greentic.msg.dlq.in.acme.slack"
/// );
/// ```
pub fn dlq_subject(direction: &str, tenant: &str, platform: &str) -> String {
    let dir = match direction {
        "in" | "out" => direction,
        _ => "in",
    };
    format!(
        "greentic.msg.dlq.{}.{}.{}",
        dir,
        norm(tenant),
        norm(platform)
    )
}

/// Subscriptions control channels (`"events"` or `"admin"`).
///
/// ```
/// use gsm_core::subs_subject;
///
/// assert_eq!(
///     subs_subject("admin", "acme", "whatsapp"),
///     "greentic.subs.admin.acme.whatsapp"
/// );
/// ```
pub fn subs_subject(kind: &str, tenant: &str, platform: &str) -> String {
    let k = match kind {
        "events" | "admin" => kind,
        _ => "events",
    };
    format!("greentic.subs.{}.{}.{}", k, norm(tenant), norm(platform))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn subjects_format() {
        assert_eq!(
            in_subject("acme", "teams", "chat/42"),
            "greentic.msg.in.acme.teams.chat-42"
        );
        assert_eq!(
            out_subject(" acme ", "tele gram", "room 7"),
            "greentic.msg.out.acme.tele-gram.room-7"
        );
        assert_eq!(
            dlq_subject("weird", "acme", "slack"),
            "greentic.msg.dlq.in.acme.slack"
        );
        assert_eq!(
            subs_subject("admin", "acme", "whatsapp"),
            "greentic.subs.admin.acme.whatsapp"
        );
    }
}
