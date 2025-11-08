use serde::{Deserialize, Serialize};
use serde_json::Value;

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MessageCard {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub footer: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ImageRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<Action>,
    #[serde(default = "default_true")]
    pub allow_markdown: bool,
    #[cfg(feature = "adaptive-cards")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adaptive: Option<Value>,
}

impl Default for MessageCard {
    fn default() -> Self {
        Self {
            title: None,
            text: None,
            footer: None,
            images: Vec::new(),
            actions: Vec::new(),
            allow_markdown: true,
            #[cfg(feature = "adaptive-cards")]
            adaptive: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ImageRef {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alt: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    OpenUrl { title: String, url: String },
    PostBack { title: String, data: Value },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_defaults_to_true() {
        let card = MessageCard::default();
        assert!(card.allow_markdown);
    }
}
