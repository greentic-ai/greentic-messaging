use std::env;

use anyhow::{Context, Result, anyhow};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ActionClaims {
    pub sub: String,
    pub tenant: String,
    pub scope: String,
    pub state_hash: String,
    pub nonce: String,
    pub exp: i64,
    pub iat: i64,
    pub jti: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redirect: Option<String>,
}

impl ActionClaims {
    /// Build a signed action request claim with a configurable expiry.
    ///
    /// ```no_run
    /// use security::jwt::{ActionClaims, JwtSigner};
    /// use time::Duration;
    ///
    /// # fn main() -> anyhow::Result<()> {
    /// unsafe { std::env::set_var("JWT_ALG", "HS256"); }
    /// unsafe { std::env::set_var("JWT_SECRET", "top-secret"); }
    /// let signer = JwtSigner::from_env()?;
    /// let claims = ActionClaims::new("room-1", "acme", "qa.submit", "hash", None, Duration::seconds(300));
    /// let token = signer.sign(&claims)?;
    /// assert!(!token.is_empty());
    /// unsafe { std::env::remove_var("JWT_SECRET"); }
    /// unsafe { std::env::remove_var("JWT_ALG"); }
    /// anyhow::Ok(())
    /// # }
    /// ```
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        sub: impl Into<String>,
        tenant: impl Into<String>,
        scope: impl Into<String>,
        state_hash: impl Into<String>,
        redirect: Option<String>,
        ttl: Duration,
    ) -> Self {
        let now = OffsetDateTime::now_utc();
        let nonce = Uuid::new_v4().to_string();
        let jti = Uuid::new_v4().to_string();
        let exp = (now + ttl).unix_timestamp();
        Self {
            sub: sub.into(),
            tenant: tenant.into(),
            scope: scope.into(),
            state_hash: state_hash.into(),
            redirect,
            nonce,
            exp,
            iat: now.unix_timestamp(),
            jti,
        }
    }

    pub fn ttl_seconds(&self) -> u64 {
        self.exp.saturating_sub(self.iat).max(1) as u64
    }
}

#[derive(Debug, Clone)]
pub struct JwtSigner {
    alg: Algorithm,
    secret: Option<Vec<u8>>,
    private_key: Option<Vec<u8>>,
    public_key: Option<Vec<u8>>,
}

impl JwtSigner {
    pub fn from_env() -> Result<Self> {
        let alg = env::var("JWT_ALG")
            .unwrap_or_else(|_| "HS256".to_string())
            .to_uppercase();
        match alg.as_str() {
            "HS256" => {
                let secret = env::var("JWT_SECRET").context("JWT_SECRET required for HS256")?;
                Ok(Self {
                    alg: Algorithm::HS256,
                    secret: Some(secret.into_bytes()),
                    private_key: None,
                    public_key: None,
                })
            }
            "RS256" => {
                let private_key =
                    env::var("JWT_PRIVATE_KEY").context("JWT_PRIVATE_KEY required for RS256")?;
                let public_key =
                    env::var("JWT_PUBLIC_KEY").context("JWT_PUBLIC_KEY required for RS256")?;
                Ok(Self {
                    alg: Algorithm::RS256,
                    secret: None,
                    private_key: Some(private_key.into_bytes()),
                    public_key: Some(public_key.into_bytes()),
                })
            }
            "ES256" => {
                let private_key =
                    env::var("JWT_PRIVATE_KEY").context("JWT_PRIVATE_KEY required for ES256")?;
                let public_key =
                    env::var("JWT_PUBLIC_KEY").context("JWT_PUBLIC_KEY required for ES256")?;
                Ok(Self {
                    alg: Algorithm::ES256,
                    secret: None,
                    private_key: Some(private_key.into_bytes()),
                    public_key: Some(public_key.into_bytes()),
                })
            }
            other => Err(anyhow!("unsupported JWT algorithm {}", other)),
        }
    }

    pub fn sign(&self, claims: &ActionClaims) -> Result<String> {
        let header = Header::new(self.alg);
        let encoding = self.encoding_key()?;
        Ok(encode(&header, claims, &encoding)?)
    }

    pub fn verify(&self, token: &str) -> Result<ActionClaims> {
        let decoding = self.decoding_key()?;
        let mut validation = Validation::new(self.alg);
        validation.validate_exp = false;
        let data = decode::<ActionClaims>(token, &decoding, &validation)?;
        Ok(data.claims)
    }

    fn encoding_key(&self) -> Result<EncodingKey> {
        match self.alg {
            Algorithm::HS256 => {
                let secret = self.secret.as_ref().context("HS256 secret missing")?;
                Ok(EncodingKey::from_secret(secret))
            }
            Algorithm::RS256 => {
                let private = self
                    .private_key
                    .as_ref()
                    .context("RS256 private key missing")?;
                Ok(EncodingKey::from_rsa_pem(private)?)
            }
            Algorithm::ES256 => {
                let private = self
                    .private_key
                    .as_ref()
                    .context("ES256 private key missing")?;
                Ok(EncodingKey::from_ec_pem(private)?)
            }
            _ => Err(anyhow!("unsupported encoding algorithm {:?}", self.alg)),
        }
    }

    fn decoding_key(&self) -> Result<DecodingKey> {
        match self.alg {
            Algorithm::HS256 => {
                let secret = self.secret.as_ref().context("HS256 secret missing")?;
                Ok(DecodingKey::from_secret(secret))
            }
            Algorithm::RS256 => {
                let public = self
                    .public_key
                    .as_ref()
                    .context("RS256 public key missing")?;
                Ok(DecodingKey::from_rsa_pem(public)?)
            }
            Algorithm::ES256 => {
                let public = self
                    .public_key
                    .as_ref()
                    .context("ES256 public key missing")?;
                Ok(DecodingKey::from_ec_pem(public)?)
            }
            _ => Err(anyhow!("unsupported decoding algorithm {:?}", self.alg)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use once_cell::sync::Lazy;
    use std::sync::Mutex;

    static ENV_GUARD: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

    #[test]
    fn hs256_roundtrip() {
        let _guard = ENV_GUARD.lock().unwrap();
        unsafe {
            std::env::set_var("JWT_ALG", "HS256");
        }
        unsafe {
            std::env::set_var("JWT_SECRET", "top-secret");
        }
        let signer = JwtSigner::from_env().expect("signer");
        let claims = ActionClaims::new(
            "chat-1",
            "acme",
            "test.scope",
            "hash",
            None,
            Duration::minutes(5),
        );
        let token = signer.sign(&claims).expect("token");
        let verified = signer.verify(&token).expect("verified");
        assert_eq!(verified.scope, claims.scope);
        assert_eq!(verified.tenant, claims.tenant);
        unsafe {
            std::env::remove_var("JWT_SECRET");
        }
        unsafe {
            std::env::remove_var("JWT_ALG");
        }
    }

    #[test]
    fn rs256_roundtrip() {
        let _guard = ENV_GUARD.lock().unwrap();
        use rand::thread_rng;
        use rsa::pkcs8::{EncodePrivateKey, EncodePublicKey};
        use rsa::{RsaPrivateKey, RsaPublicKey};

        let mut rng = thread_rng();
        let private_key = RsaPrivateKey::new(&mut rng, 2048).expect("generate rsa key");
        let public_key = RsaPublicKey::from(&private_key);
        let private_pem = private_key
            .to_pkcs8_pem(rsa::pkcs8::LineEnding::LF)
            .expect("encode private")
            .to_string();
        let public_pem = public_key
            .to_public_key_pem(rsa::pkcs8::LineEnding::LF)
            .expect("encode public")
            .to_string();

        unsafe {
            std::env::set_var("JWT_ALG", "RS256");
        }
        unsafe {
            std::env::set_var("JWT_PRIVATE_KEY", private_pem);
        }
        unsafe {
            std::env::set_var("JWT_PUBLIC_KEY", public_pem);
        }
        let signer = JwtSigner::from_env().expect("signer");
        let claims = ActionClaims::new(
            "chat-9",
            "bravo",
            "test.scope",
            "hash",
            None,
            Duration::minutes(5),
        );
        let token = signer.sign(&claims).expect("token");
        let verified = signer.verify(&token).expect("verified");
        assert_eq!(verified.scope, claims.scope);
        unsafe {
            std::env::remove_var("JWT_PRIVATE_KEY");
        }
        unsafe {
            std::env::remove_var("JWT_PUBLIC_KEY");
        }
        unsafe {
            std::env::remove_var("JWT_ALG");
        }
    }
}
