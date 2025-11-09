use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::messaging_card::ir::MessageCardIr;
use crate::messaging_card::types::{MessageCard, OauthCard, OauthPrompt, OauthProvider};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RenderIntent {
    Card,
    Auth,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RenderSpec {
    Card(MessageCardIr),
    Auth(AuthRenderSpec),
}

impl RenderSpec {
    pub fn intent(&self) -> RenderIntent {
        match self {
            RenderSpec::Card(_) => RenderIntent::Card,
            RenderSpec::Auth(_) => RenderIntent::Auth,
        }
    }

    pub fn as_card(&self) -> Option<&MessageCardIr> {
        match self {
            RenderSpec::Card(ir) => Some(ir),
            _ => None,
        }
    }

    pub fn as_auth(&self) -> Option<&AuthRenderSpec> {
        match self {
            RenderSpec::Auth(spec) => Some(spec),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuthRenderSpec {
    pub provider: OauthProvider,
    pub scopes: Vec<String>,
    pub resource: Option<String>,
    pub prompt: Option<OauthPrompt>,
    pub metadata: Option<Value>,
    pub start_url: Option<String>,
    pub connection_name: Option<String>,
    pub fallback_button: FallbackButton,
}

impl AuthRenderSpec {
    pub fn from_card(card: &MessageCard, oauth: &OauthCard) -> Self {
        let fallback_title = card
            .title
            .clone()
            .unwrap_or_else(|| format!("Sign in with {}", oauth.provider.display_name()));
        let fallback_button = FallbackButton {
            title: fallback_title,
            url: oauth.start_url.clone(),
        };
        Self {
            provider: oauth.provider.clone(),
            scopes: oauth.scopes.clone(),
            resource: oauth.resource.clone(),
            prompt: oauth.prompt.clone(),
            metadata: oauth.metadata.clone(),
            start_url: oauth.start_url.clone(),
            connection_name: oauth.connection_name.clone(),
            fallback_button,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FallbackButton {
    pub title: String,
    pub url: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messaging_card::types::{MessageCardKind, OauthPrompt};

    #[test]
    fn auth_spec_defaults_title() {
        let oauth = OauthCard {
            provider: OauthProvider::Microsoft,
            scopes: vec!["User.Read".into()],
            resource: None,
            prompt: Some(OauthPrompt::Consent),
            start_url: Some("https://auth/start".into()),
            connection_name: Some("graph".into()),
            metadata: None,
        };
        let card = MessageCard {
            kind: MessageCardKind::Oauth,
            title: None,
            text: None,
            footer: None,
            images: Vec::new(),
            actions: Vec::new(),
            allow_markdown: true,
            #[cfg(feature = "adaptive-cards")]
            adaptive: None,
            oauth: Some(oauth.clone()),
        };

        let spec = AuthRenderSpec::from_card(&card, card.oauth.as_ref().unwrap());
        assert_eq!(spec.provider, OauthProvider::Microsoft);
        assert_eq!(spec.scopes, vec!["User.Read".to_string()]);
        assert_eq!(
            spec.fallback_button.title,
            "Sign in with Microsoft".to_string()
        );
        assert_eq!(
            spec.fallback_button.url,
            Some("https://auth/start".to_string())
        );
        assert_eq!(spec.connection_name.as_deref(), Some("graph"));
        assert_eq!(spec.prompt, Some(OauthPrompt::Consent));
    }
}
