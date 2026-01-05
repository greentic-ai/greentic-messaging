use anyhow::{Result, anyhow};
use gsm_ingress_common::webex::SignatureAlgorithm;
use hmac::{Hmac, Mac};
use sha1::Sha1;
use sha2::Sha256;
use subtle::ConstantTimeEq;

type HmacSha1 = Hmac<Sha1>;
type HmacSha256 = Hmac<Sha256>;

/// Compute the expected signature for the provided payload and secret.
pub(crate) fn compute_signature(
    secret: &str,
    body: &[u8],
    algo: SignatureAlgorithm,
) -> Result<Vec<u8>> {
    let key = secret.as_bytes();
    let digest = match algo {
        SignatureAlgorithm::Sha1 => {
            let mut mac =
                HmacSha1::new_from_slice(key).map_err(|e| anyhow!("invalid HMAC key: {e}"))?;
            mac.update(body);
            mac.finalize().into_bytes().to_vec()
        }
        SignatureAlgorithm::Sha256 => {
            let mut mac =
                HmacSha256::new_from_slice(key).map_err(|e| anyhow!("invalid HMAC key: {e}"))?;
            mac.update(body);
            mac.finalize().into_bytes().to_vec()
        }
    };
    Ok(digest)
}

fn parse_signature(signature: &str) -> Result<Vec<u8>> {
    let trimmed = signature.trim();
    let value = trimmed
        .strip_prefix("sha1=")
        .or_else(|| trimmed.strip_prefix("sha256="))
        .unwrap_or(trimmed);
    hex::decode(value).map_err(|e| anyhow!("invalid signature hex: {e}"))
}

/// Verifies a Webex webhook signature. Returns `true` when the payload matches the provided signature.
pub fn verify_signature(
    secret: &str,
    signature: &str,
    body: &[u8],
    algo: SignatureAlgorithm,
) -> Result<bool> {
    let provided = parse_signature(signature)?;
    let expected = compute_signature(secret, body, algo)?;
    Ok(provided.len() == expected.len() && provided.ct_eq(&expected).into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::RngCore;

    fn random_secret() -> String {
        let mut buf = [0u8; 32];
        let mut rng = rand::rng();
        rng.fill_bytes(&mut buf);
        hex::encode(buf)
    }

    #[test]
    fn verifies_sha1_signature() {
        let secret = random_secret();
        let body = br#"{"hello":"world"}"#;
        let sig = compute_signature(&secret, body, SignatureAlgorithm::Sha1).unwrap();
        let signature = format!("sha1={}", hex::encode(sig));
        assert!(verify_signature(&secret, &signature, body, SignatureAlgorithm::Sha1).unwrap());
    }

    #[test]
    fn rejects_invalid_signature() {
        let secret = random_secret();
        let other_secret = random_secret();
        let body = br#"{"hello":"world"}"#;
        let sig = compute_signature(&other_secret, body, SignatureAlgorithm::Sha1).unwrap();
        let signature = format!("sha1={}", hex::encode(sig));
        let valid = verify_signature(&secret, &signature, body, SignatureAlgorithm::Sha1)
            .expect("verification");
        assert!(!valid);
    }
}
