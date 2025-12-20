use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ProviderCapabilitiesV1 {
    pub version: String,
    pub supports_adaptive_cards: bool,
    pub supports_markdown: bool,
    pub supports_html: bool,
    pub supports_images: bool,
    pub supports_buttons: bool,
    pub supports_threads: bool,
    pub max_text_len: Option<u32>,
    pub max_payload_bytes: Option<u32>,
    pub max_actions: Option<u32>,
    pub max_buttons_per_row: Option<u32>,
    pub max_total_buttons: Option<u32>,
    #[serde(default)]
    pub limits: ProviderLimitsV1,
}

impl Default for ProviderCapabilitiesV1 {
    fn default() -> Self {
        Self {
            version: "v1".to_string(),
            supports_adaptive_cards: false,
            supports_markdown: false,
            supports_html: false,
            supports_images: false,
            supports_buttons: false,
            supports_threads: false,
            max_text_len: None,
            max_payload_bytes: None,
            max_actions: None,
            max_buttons_per_row: None,
            max_total_buttons: None,
            limits: ProviderLimitsV1::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub struct ProviderLimitsV1 {
    pub max_text_len: Option<u32>,
    pub max_payload_bytes: Option<u32>,
    pub max_actions: Option<u32>,
    pub max_buttons_per_row: Option<u32>,
    pub max_total_buttons: Option<u32>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CapabilitiesError {
    #[error("version must be \"v1\"")]
    BadVersion,
    #[error("max_buttons_per_row cannot exceed max_total_buttons")]
    ButtonsRowExceedsTotal,
}

impl ProviderCapabilitiesV1 {
    pub fn validate(&self) -> Result<(), CapabilitiesError> {
        if self.version != "v1" {
            return Err(CapabilitiesError::BadVersion);
        }

        let row = self.limits.max_buttons_per_row.or(self.max_buttons_per_row);
        let total = self.limits.max_total_buttons.or(self.max_total_buttons);
        if let (Some(r), Some(t)) = (row, total)
            && r > t
        {
            return Err(CapabilitiesError::ButtonsRowExceedsTotal);
        }

        Ok(())
    }
}
