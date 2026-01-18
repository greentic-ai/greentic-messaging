//! Canonical NATS subject helpers for messaging ingress/egress.

use std::borrow::Cow;

pub const INGRESS_SUBJECT_PREFIX: &str = "greentic.messaging.ingress";
pub const EGRESS_SUBJECT_PREFIX: &str = "greentic.messaging.egress";

fn norm<S: AsRef<str>>(s: S) -> Cow<'static, str> {
    let mut value = s
        .as_ref()
        .trim()
        .replace([' ', '\t', '\n', '\r', '*', '>', '/'], "-");
    if value.is_empty() {
        value = "unknown".into();
    }
    Cow::Owned(value)
}

pub fn ingress_subject(env: &str, tenant: &str, team: &str, platform: &str) -> String {
    ingress_subject_with_prefix(INGRESS_SUBJECT_PREFIX, env, tenant, team, platform)
}

pub fn ingress_subject_with_prefix(
    prefix: &str,
    env: &str,
    tenant: &str,
    team: &str,
    platform: &str,
) -> String {
    format!(
        "{prefix}.{}.{}.{}.{}",
        norm(env),
        norm(tenant),
        norm(team),
        norm(platform)
    )
}

pub fn egress_subject(env: &str, tenant: &str, team: &str, platform: &str) -> String {
    egress_subject_with_prefix(EGRESS_SUBJECT_PREFIX, env, tenant, team, platform)
}

pub fn egress_subject_with_prefix(
    prefix: &str,
    env: &str,
    tenant: &str,
    team: &str,
    platform: &str,
) -> String {
    format!(
        "{prefix}.{}.{}.{}.{}",
        norm(env),
        norm(tenant),
        norm(team),
        norm(platform)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subjects_are_deterministic_and_sanitized() {
        assert_eq!(
            ingress_subject("dev", "acme", "team", "slack"),
            "greentic.messaging.ingress.dev.acme.team.slack"
        );
        assert_eq!(
            egress_subject(" dev ", "acme", "team a", "web chat"),
            "greentic.messaging.egress.dev.acme.team-a.web-chat"
        );
    }
}
