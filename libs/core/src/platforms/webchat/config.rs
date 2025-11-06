const DEFAULT_DIRECT_LINE_BASE: &str = "https://directline.botframework.com/v3/directline";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Config {
    direct_line_base: String,
}

impl Config {
    pub fn new(direct_line_base: impl Into<String>) -> Self {
        Self {
            direct_line_base: direct_line_base.into(),
        }
    }

    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        Self::new(base_url)
    }

    pub fn direct_line_base(&self) -> &str {
        &self.direct_line_base
    }
}

impl Default for Config {
    fn default() -> Self {
        Self::new(DEFAULT_DIRECT_LINE_BASE)
    }
}

#[derive(Debug, Clone)]
pub struct SigningKeys {
    pub secret: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OAuthProviderConfig {
    pub issuer: String,
    pub client_id: String,
    pub redirect_base: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_base_url() {
        let config = Config::default();
        assert_eq!(config.direct_line_base(), DEFAULT_DIRECT_LINE_BASE);
    }

    #[test]
    fn custom_base_url() {
        let config = Config::with_base_url("https://example.com/directline");
        assert_eq!(config.direct_line_base(), "https://example.com/directline");
    }
}
