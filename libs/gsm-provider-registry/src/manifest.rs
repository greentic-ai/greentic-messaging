use serde::Deserialize;

/// Errors produced while parsing or validating a provider manifest.
#[derive(thiserror::Error, Debug)]
pub enum ManifestError {
    #[error("manifest parse error: {0}")]
    Parse(#[from] serde_json::Error),
    #[error("manifest failed schema validation: {0}")]
    Invalid(String),
}

/// Manifest describing a connector provider.
#[derive(Clone, Debug, Deserialize)]
pub struct ProviderManifest {
    pub name: String,
    pub version: String,
    pub kind: String,
    pub auth: AuthConfig,
    pub capabilities: Capabilities,
    pub endpoints: Endpoints,
}

#[derive(Clone, Debug, Deserialize)]
pub struct AuthConfig {
    #[serde(rename = "type")]
    pub auth_type: String,
    #[serde(default)]
    pub secrets: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Capabilities {
    pub supports_threads: bool,
    pub attachments: bool,
    pub max_text_len: u32,
    pub rate_limit: RateLimit,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RateLimit {
    pub rpm: u32,
    pub burst: u32,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Endpoints {
    pub send: String,
    #[serde(default)]
    pub receive_webhook: Option<String>,
}

impl ProviderManifest {
    /// Parses and validates a manifest from a JSON string.
    pub fn from_json(content: &str) -> Result<Self, ManifestError> {
        let manifest: ProviderManifest = serde_json::from_str(content)?;
        manifest.ensure_valid()?;
        Ok(manifest)
    }

    fn ensure_valid(&self) -> Result<(), ManifestError> {
        if self.name.trim().is_empty() {
            return Err(ManifestError::Invalid("name must not be empty".into()));
        }
        if self.version.trim().is_empty() {
            return Err(ManifestError::Invalid("version must not be empty".into()));
        }
        if self.kind.trim().is_empty() {
            return Err(ManifestError::Invalid("kind must not be empty".into()));
        }
        if self.endpoints.send.trim().is_empty() {
            return Err(ManifestError::Invalid(
                "endpoints.send must not be empty".into(),
            ));
        }
        if self.capabilities.max_text_len == 0 {
            return Err(ManifestError::Invalid(
                "capabilities.max_text_len must be positive".into(),
            ));
        }
        if self.capabilities.rate_limit.rpm == 0 || self.capabilities.rate_limit.burst == 0 {
            return Err(ManifestError::Invalid(
                "capabilities.rate_limit values must be positive".into(),
            ));
        }
        Ok(())
    }
}
