use anyhow::Result;
use time::Duration;

use crate::jwt::{ActionClaims, JwtSigner};

pub fn action_base_url() -> Option<String> {
    std::env::var("ACTION_BASE_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
}

pub fn action_ttl() -> Duration {
    let seconds = std::env::var("ACTION_TTL_SECONDS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(900);
    Duration::seconds(seconds as i64)
}

pub fn build_action_url(base: &str, claims: ActionClaims, signer: &JwtSigner) -> Result<String> {
    let token = signer.sign(&claims)?;
    let mut normalized = base.trim_end_matches('&').trim_end_matches('?').to_string();
    if !normalized.contains('?') {
        normalized.push_str("?action=");
    } else if normalized.ends_with('?') || normalized.ends_with('&') {
        normalized.push_str("action=");
    } else {
        normalized.push_str("&action=");
    }
    normalized.push_str(&urlencoding::encode(&token));
    Ok(normalized)
}

pub fn claims_with_redirect(
    sub: impl Into<String>,
    tenant: impl Into<String>,
    scope: impl Into<String>,
    state_hash: impl Into<String>,
    redirect: impl Into<String>,
    ttl: Duration,
) -> ActionClaims {
    ActionClaims::new(sub, tenant, scope, state_hash, Some(redirect.into()), ttl)
}
