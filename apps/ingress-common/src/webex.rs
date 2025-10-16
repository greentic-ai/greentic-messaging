use std::env;

/// Default HTTP header carrying the Webex webhook signature.
pub const DEFAULT_SIGNATURE_HEADER: &str = "X-Webex-Signature";
/// Environment variable overriding the signature header name.
pub const SIG_HEADER_ENV: &str = "WEBEX_SIG_HEADER";
/// Environment variable selecting the HMAC algorithm (`sha1` or `sha256`).
pub const SIG_ALGO_ENV: &str = "WEBEX_SIG_ALGO";

/// Supported HMAC algorithms for Webex webhook verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureAlgorithm {
    Sha1,
    Sha256,
}

impl SignatureAlgorithm {
    /// Resolve the signature algorithm from the `WEBEX_SIG_ALGO` environment variable.
    /// Defaults to `sha1`, the value used by Webex webhooks today.
    pub fn from_env() -> Self {
        match env::var(SIG_ALGO_ENV)
            .unwrap_or_else(|_| "sha1".to_string())
            .to_lowercase()
            .as_str()
        {
            "sha256" => SignatureAlgorithm::Sha256,
            _ => SignatureAlgorithm::Sha1,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            SignatureAlgorithm::Sha1 => "sha1",
            SignatureAlgorithm::Sha256 => "sha256",
        }
    }
}

/// Resolve the HTTP header used for signatures, defaulting to `X-Webex-Signature`.
pub fn signature_header_from_env() -> String {
    env::var(SIG_HEADER_ENV).unwrap_or_else(|_| DEFAULT_SIGNATURE_HEADER.to_string())
}
