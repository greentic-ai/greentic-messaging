/// Default HTTP header carrying the Webex webhook signature.
pub const DEFAULT_SIGNATURE_HEADER: &str = "X-Webex-Signature";

/// Supported HMAC algorithms for Webex webhook verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureAlgorithm {
    Sha1,
    Sha256,
}

impl SignatureAlgorithm {
    /// Default signature algorithm for Webex webhooks.
    pub fn default_algo() -> Self {
        SignatureAlgorithm::Sha1
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            SignatureAlgorithm::Sha1 => "sha1",
            SignatureAlgorithm::Sha256 => "sha256",
        }
    }
}

/// Resolve the HTTP header used for signatures, defaulting to `X-Webex-Signature`.
pub fn signature_header() -> &'static str {
    DEFAULT_SIGNATURE_HEADER
}
