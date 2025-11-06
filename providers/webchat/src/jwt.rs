//! JWT helpers for the standalone Direct Line feature.
//!
//! Implements HS256 signing/verification using configuration supplied by
//! `Config`. Tokens embed tenant context information so that the Direct Line
//! server can authorise conversation access without contacting the upstream
//! Microsoft service.

use std::sync::Arc;

use anyhow::Context;
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use once_cell::sync::OnceCell;
use serde::{Deserialize, Serialize};
use time::{Duration as TimeDuration, OffsetDateTime};

use crate::config::SigningKeys;

const ISSUER: &str = "greentic.webchat";
const AUDIENCE: &str = "directline";

/// Encoded tenant context inside Direct Line tokens.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TenantClaims {
    pub env: String,
    pub tenant: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub team: Option<String>,
}

/// JWT payload used for Direct Line tokens.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Claims {
    pub iss: String,
    pub aud: String,
    pub sub: String,
    pub exp: i64,
    pub iat: i64,
    pub nbf: i64,
    pub ctx: TenantClaims,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conv: Option<String>,
}

impl Claims {
    /// Creates a new claim with the provided subject and tenant context.
    pub fn new(sub: String, ctx: TenantClaims, ttl: TimeDuration) -> Self {
        let now = OffsetDateTime::now_utc();
        let exp = now + ttl;
        Self {
            iss: ISSUER.into(),
            aud: AUDIENCE.into(),
            sub,
            exp: exp.unix_timestamp(),
            iat: now.unix_timestamp(),
            nbf: now.unix_timestamp(),
            ctx,
            conv: None,
        }
    }

    /// Returns a copy bound to the supplied conversation identifier.
    pub fn with_conversation(mut self, conversation_id: impl Into<String>) -> Self {
        self.conv = Some(conversation_id.into());
        self
    }

    /// Returns true when the claim has been bound to a conversation.
    pub fn has_conversation(&self, conversation_id: &str) -> bool {
        self.conv
            .as_ref()
            .map(|conv| conv.eq_ignore_ascii_case(conversation_id))
            .unwrap_or(false)
    }
}

/// Signing/verification entry point. Lazily initialised from configuration.
#[derive(Clone)]
pub struct JwtKeys {
    encoding: EncodingKey,
    decoding: DecodingKey,
}

impl JwtKeys {
    fn from_config(keys: &SigningKeys) -> anyhow::Result<Self> {
        let encoding = EncodingKey::from_secret(keys.secret.as_bytes());
        let decoding = DecodingKey::from_secret(keys.secret.as_bytes());
        Ok(Self { encoding, decoding })
    }
}

static ACTIVE_KEYS: OnceCell<Arc<JwtKeys>> = OnceCell::new();

/// Installs the JWT signing keys from configuration.
pub fn install_keys(keys: SigningKeys) -> anyhow::Result<()> {
    ACTIVE_KEYS
        .set(Arc::new(JwtKeys::from_config(&keys)?))
        .map_err(|_| anyhow::anyhow!("JWT keys have already been installed"))
}

fn active_keys() -> anyhow::Result<Arc<JwtKeys>> {
    ACTIVE_KEYS
        .get()
        .cloned()
        .context("JWT signing keys not initialised")
}

/// Serialises and signs the supplied claims returning the encoded JWT.
pub fn sign(claims: &Claims) -> anyhow::Result<String> {
    let keys = active_keys()?;
    let mut header = Header::default();
    header.alg = Algorithm::HS256;
    let token = jsonwebtoken::encode(&header, claims, &keys.encoding)?;
    Ok(token)
}

/// Validates a token and returns the decoded claims.
pub fn verify(token: &str) -> anyhow::Result<Claims> {
    let keys = active_keys()?;
    let mut validation = Validation::new(Algorithm::HS256);
    validation.set_audience(&[AUDIENCE]);
    validation.set_issuer(&[ISSUER]);
    validation.leeway = 5; // seconds
    let data = jsonwebtoken::decode::<Claims>(token, &keys.decoding, &validation)?;
    Ok(data.claims)
}

/// Convenience for converting a chrono-like duration into seconds.
pub fn ttl(seconds: u64) -> TimeDuration {
    TimeDuration::seconds(seconds as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn install_test_keys() {
        static INSTALL: once_cell::sync::OnceCell<()> = once_cell::sync::OnceCell::new();
        INSTALL.get_or_init(|| {
            install_keys(SigningKeys {
                secret: "test-signing-key".into(),
            })
            .expect("install keys")
        });
    }

    #[test]
    fn sign_and_verify_round_trip() {
        install_test_keys();
        let claims = Claims::new(
            "user-1".into(),
            TenantClaims {
                env: "dev".into(),
                tenant: "acme".into(),
                team: Some("support".into()),
            },
            ttl(600),
        )
        .with_conversation("conv-42");
        let token = sign(&claims).expect("token");
        let parsed = verify(&token).expect("verify");
        assert_eq!(parsed.sub, claims.sub);
        assert_eq!(parsed.ctx.tenant, claims.ctx.tenant);
        assert_eq!(parsed.conv, Some("conv-42".into()));
        assert!(parsed.exp >= parsed.iat);
    }
}
