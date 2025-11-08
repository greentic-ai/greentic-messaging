use serde::{Deserialize, Serialize};
use serde_json::Value;

pub mod normalizer;
pub mod validator;

pub use validator::{ValidateError, validate_ac_json};

/// Supported Adaptive Card schema versions for the bootstrap phase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum AdaptiveCardVersion {
    #[default]
    V1_6,
    Custom(String),
}

/// Lightweight wrapper that keeps the original Adaptive Card JSON around the pipeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct AdaptiveCardPayload {
    #[serde(default)]
    pub version: AdaptiveCardVersion,
    #[serde(default)]
    pub content: Value,
}

impl AdaptiveCardPayload {
    pub fn new(content: Value) -> Self {
        Self {
            content,
            ..Default::default()
        }
    }
}
