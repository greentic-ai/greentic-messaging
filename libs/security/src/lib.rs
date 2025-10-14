use anyhow::Result;
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use time::{Duration, OffsetDateTime};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TokenClaims {
    pub subject: String,
    pub expires_at: i64,
}

impl TokenClaims {
    pub fn new(subject: impl Into<String>, ttl: Duration) -> Self {
        let expires_at = (OffsetDateTime::now_utc() + ttl).unix_timestamp();
        Self {
            subject: subject.into(),
            expires_at,
        }
    }
}

pub fn sign(claims: &TokenClaims, secret: &str) -> Result<String> {
    let mut header = Header::new(Algorithm::HS256);
    header.kid = Some("greentic".into());
    Ok(encode(
        &header,
        claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )?)
}

pub fn verify(token: &str, secret: &str) -> Result<TokenClaims> {
    let mut validation = Validation::new(Algorithm::HS256);
    validation.validate_exp = false;
    validation.required_spec_claims.remove("exp");
    Ok(decode::<TokenClaims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &validation,
    )?
    .claims)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_claims_new_sets_future_expiry() {
        let ttl = Duration::minutes(5);
        let claims = TokenClaims::new("user-123", ttl);
        let now = OffsetDateTime::now_utc().unix_timestamp();
        assert!(claims.expires_at >= now);
        assert!(claims.expires_at <= now + ttl.whole_seconds());
        assert_eq!(claims.subject, "user-123");
    }

    #[test]
    fn sign_and_verify_round_trip() {
        let secret = "top-secret";
        let claims = TokenClaims::new("user-123", Duration::minutes(1));
        let token = sign(&claims, secret).expect("token");
        let verified = verify(&token, secret).expect("verify");
        assert_eq!(verified.subject, "user-123");
    }

    #[test]
    fn verify_fails_with_wrong_secret() {
        let claims = TokenClaims::new("user-123", Duration::minutes(1));
        let token = sign(&claims, "good-secret").expect("token");
        assert!(verify(&token, "bad-secret").is_err());
    }
}
