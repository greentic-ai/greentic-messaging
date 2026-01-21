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
    /// use security::jwt::{ActionClaims, JwtConfig, JwtSigner};
    /// use time::Duration;
    ///
    /// # fn main() -> anyhow::Result<()> {
    /// let signer = JwtSigner::from_config(JwtConfig::hs256("top-secret"))?;
    /// let claims = ActionClaims::new("room-1", "acme", "qa.submit", "hash", None, Duration::seconds(300));
    /// let token = signer.sign(&claims)?;
    /// assert!(!token.is_empty());
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

#[derive(Debug, Clone)]
pub struct JwtConfig {
    alg: Algorithm,
    secret: Option<Vec<u8>>,
    private_key: Option<Vec<u8>>,
    public_key: Option<Vec<u8>>,
}

impl JwtConfig {
    pub fn hs256(secret: impl Into<Vec<u8>>) -> Self {
        Self {
            alg: Algorithm::HS256,
            secret: Some(secret.into()),
            private_key: None,
            public_key: None,
        }
    }

    pub fn rs256(private_key: impl Into<Vec<u8>>, public_key: impl Into<Vec<u8>>) -> Self {
        Self {
            alg: Algorithm::RS256,
            secret: None,
            private_key: Some(private_key.into()),
            public_key: Some(public_key.into()),
        }
    }

    pub fn es256(private_key: impl Into<Vec<u8>>, public_key: impl Into<Vec<u8>>) -> Self {
        Self {
            alg: Algorithm::ES256,
            secret: None,
            private_key: Some(private_key.into()),
            public_key: Some(public_key.into()),
        }
    }
}

impl JwtSigner {
    pub fn from_config(config: JwtConfig) -> Result<Self> {
        match config.alg {
            Algorithm::HS256 => {
                let secret = config.secret.context("HS256 secret missing")?;
                if secret.is_empty() {
                    return Err(anyhow!("HS256 secret missing"));
                }
                Ok(Self {
                    alg: Algorithm::HS256,
                    secret: Some(secret),
                    private_key: None,
                    public_key: None,
                })
            }
            Algorithm::RS256 => {
                let private_key = config.private_key.context("RS256 private key missing")?;
                let public_key = config.public_key.context("RS256 public key missing")?;
                Ok(Self {
                    alg: Algorithm::RS256,
                    secret: None,
                    private_key: Some(private_key),
                    public_key: Some(public_key),
                })
            }
            Algorithm::ES256 => {
                let private_key = config.private_key.context("ES256 private key missing")?;
                let public_key = config.public_key.context("ES256 public key missing")?;
                Ok(Self {
                    alg: Algorithm::ES256,
                    secret: None,
                    private_key: Some(private_key),
                    public_key: Some(public_key),
                })
            }
            other => Err(anyhow!("unsupported JWT algorithm {:?}", other)),
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

    #[test]
    fn hs256_roundtrip() {
        let signer = JwtSigner::from_config(JwtConfig::hs256("top-secret")).expect("signer");
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
    }

    #[test]
    fn rs256_roundtrip() {
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
        let signer =
            JwtSigner::from_config(JwtConfig::rs256(private_pem, public_pem)).expect("signer");
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
    }
}
