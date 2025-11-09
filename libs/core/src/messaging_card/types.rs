use serde::{Deserialize, Serialize};
use serde_json::Value;

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MessageCard {
    #[serde(default)]
    pub kind: MessageCardKind,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oauth: Option<OauthCard>,
}

impl Default for MessageCard {
    fn default() -> Self {
        Self {
            kind: MessageCardKind::default(),
            title: None,
            text: None,
            footer: None,
            images: Vec::new(),
            actions: Vec::new(),
            allow_markdown: true,
            #[cfg(feature = "adaptive-cards")]
            adaptive: None,
            oauth: None,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MessageCardKind {
    Standard,
    Oauth,
}

impl Default for MessageCardKind {
    fn default() -> Self {
        Self::Standard
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OauthProvider {
    Microsoft,
    Google,
    Github,
    Custom,
}

impl OauthProvider {
    pub fn as_str(&self) -> &'static str {
        match self {
            OauthProvider::Microsoft => "microsoft",
            OauthProvider::Google => "google",
            OauthProvider::Github => "github",
            OauthProvider::Custom => "custom",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            OauthProvider::Microsoft => "Microsoft",
            OauthProvider::Google => "Google",
            OauthProvider::Github => "GitHub",
            OauthProvider::Custom => "External Provider",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OauthPrompt {
    None,
    Consent,
    Login,
}

impl OauthPrompt {
    pub fn as_str(&self) -> &'static str {
        match self {
            OauthPrompt::None => "none",
            OauthPrompt::Consent => "consent",
            OauthPrompt::Login => "login",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OauthCard {
    pub provider: OauthProvider,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scopes: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<OauthPrompt>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn markdown_defaults_to_true() {
        let card = MessageCard::default();
        assert!(card.allow_markdown);
    }

    #[test]
    fn message_card_defaults_to_standard_kind() {
        let card = MessageCard::default();
        assert!(matches!(card.kind, MessageCardKind::Standard));
        assert!(card.oauth.is_none());
    }

    #[test]
    fn oauth_card_round_trip() {
        let oauth = OauthCard {
            provider: OauthProvider::Microsoft,
            scopes: vec!["User.Read".into()],
            resource: Some("https://graph.microsoft.com".into()),
            prompt: Some(OauthPrompt::Consent),
            start_url: Some("https://oauth/start".into()),
            connection_name: Some("m365".into()),
            metadata: Some(json!({"tenant":"acme"})),
        };
        let card = MessageCard {
            kind: MessageCardKind::Oauth,
            title: Some("Auth".into()),
            oauth: Some(oauth),
            ..Default::default()
        };

        let value = serde_json::to_value(&card).expect("serialize");
        let restored: MessageCard = serde_json::from_value(value).expect("deserialize");
        assert!(matches!(restored.kind, MessageCardKind::Oauth));
        let oauth = restored.oauth.expect("oauth payload");
        assert_eq!(oauth.scopes, vec!["User.Read".to_string()]);
    }
}
