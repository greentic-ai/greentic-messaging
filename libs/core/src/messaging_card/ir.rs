use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::messaging_card::tier::Tier;
use crate::messaging_card::types::{Action, MessageCard};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MessageCardIr {
    pub tier: Tier,
    pub head: Head,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub elements: Vec<Element>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<IrAction>,
    #[serde(default)]
    pub meta: Meta,
}

impl Default for MessageCardIr {
    fn default() -> Self {
        Self {
            tier: Tier::Basic,
            head: Head::default(),
            elements: Vec::new(),
            actions: Vec::new(),
            meta: Meta::default(),
        }
    }
}

impl MessageCardIr {
    pub fn from_plain(card: &MessageCard) -> Self {
        let mut builder = MessageCardIrBuilder::default().tier(Tier::Basic);

        if let Some(title) = &card.title {
            builder = builder.title(title);
        }
        if let Some(text) = &card.text {
            builder = builder.primary_text(text, card.allow_markdown);
        }
        if let Some(footer) = &card.footer {
            builder = builder.footer(footer);
        }
        for image in &card.images {
            builder = builder.image(image.url.clone(), image.alt.clone());
        }
        for action in &card.actions {
            builder = match action {
                Action::OpenUrl { title, url } => builder.open_url(title, url),
                Action::PostBack { title, data } => builder.postback(title, data.clone()),
            };
        }

        let mut built = builder.build();
        built.auto_tier();
        built
    }

    pub fn auto_tier(&mut self) {
        self.tier = self.derive_tier();
    }

    fn derive_tier(&self) -> Tier {
        let premium = self
            .elements
            .iter()
            .any(|element| matches!(element, Element::Input { .. }))
            || self
                .meta
                .capabilities
                .iter()
                .any(|cap| matches!(cap.as_str(), "inputs" | "execute" | "showcard"));
        if premium {
            return Tier::Premium;
        }

        let advanced = self
            .elements
            .iter()
            .any(|element| matches!(element, Element::Image { .. } | Element::FactSet { .. }))
            || self
                .actions
                .iter()
                .any(|action| matches!(action, IrAction::Postback { .. }));

        if advanced {
            Tier::Advanced
        } else {
            Tier::Basic
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Head {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub footer: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Element {
    Text {
        text: String,
        markdown: bool,
    },
    Image {
        url: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        alt: Option<String>,
    },
    FactSet {
        facts: Vec<Fact>,
    },
    Input {
        #[serde(skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        kind: InputKind,
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        required: bool,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        choices: Vec<InputChoice>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Fact {
    pub label: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum InputKind {
    Text,
    Choice,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InputChoice {
    pub title: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IrAction {
    OpenUrl { title: String, url: String },
    Postback { title: String, data: Value },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Meta {
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub capabilities: BTreeSet<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adaptive_payload: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_link: Option<AppLink>,
}

impl Meta {
    pub fn add_capability(&mut self, cap: impl Into<String>) {
        self.capabilities.insert(cap.into());
    }

    pub fn warn(&mut self, message: impl Into<String>) {
        self.warnings.push(message.into());
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AppLink {
    pub base_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tenant: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jwt: Option<AppLinkJwt>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AppLinkJwt {
    pub secret: String,
    #[serde(default = "default_app_link_jwt_algorithm")]
    pub algorithm: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audience: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issuer: Option<String>,
    #[serde(default = "default_app_link_jwt_ttl")]
    pub ttl_seconds: u64,
}

fn default_app_link_jwt_algorithm() -> String {
    "HS256".into()
}

fn default_app_link_jwt_ttl() -> u64 {
    900
}

#[derive(Debug, Default)]
pub struct MessageCardIrBuilder {
    inner: MessageCardIr,
}

impl MessageCardIrBuilder {
    pub fn tier(mut self, tier: Tier) -> Self {
        self.inner.tier = tier;
        self
    }

    pub fn title(mut self, title: &str) -> Self {
        self.inner.head.title = Some(title.to_string());
        self
    }

    pub fn primary_text(mut self, text: &str, markdown: bool) -> Self {
        self.inner.head.text = Some(text.to_string());
        self.inner.elements.push(Element::Text {
            text: text.into(),
            markdown,
        });
        self
    }

    pub fn footer(mut self, footer: &str) -> Self {
        self.inner.head.footer = Some(footer.to_string());
        self
    }

    pub fn image(mut self, url: String, alt: Option<String>) -> Self {
        self.inner.elements.push(Element::Image { url, alt });
        self
    }

    pub fn fact(mut self, label: &str, value: &str) -> Self {
        self.inner.elements.push(Element::FactSet {
            facts: vec![Fact {
                label: label.into(),
                value: value.into(),
            }],
        });
        self
    }

    pub fn input(
        mut self,
        label: Option<String>,
        kind: InputKind,
        id: Option<String>,
        choices: Vec<InputChoice>,
    ) -> Self {
        self.inner.elements.push(Element::Input {
            label,
            kind,
            id,
            required: false,
            choices,
        });
        self
    }

    pub fn open_url(mut self, title: &str, url: &str) -> Self {
        self.inner.actions.push(IrAction::OpenUrl {
            title: title.into(),
            url: url.into(),
        });
        self
    }

    pub fn postback(mut self, title: &str, data: Value) -> Self {
        self.inner.actions.push(IrAction::Postback {
            title: title.into(),
            data,
        });
        self
    }

    pub fn build(self) -> MessageCardIr {
        self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messaging_card::types::{Action, ImageRef, MessageCard};
    use serde_json::json;

    #[test]
    fn builder_translates_plain_card() {
        let card = MessageCard {
            title: Some("IR".into()),
            text: Some("hello".into()),
            footer: Some("footer".into()),
            images: vec![ImageRef {
                url: "https://example.com/img.png".into(),
                alt: Some("img".into()),
            }],
            actions: vec![
                Action::OpenUrl {
                    title: "view".into(),
                    url: "https://example.com".into(),
                },
                Action::PostBack {
                    title: "ack".into(),
                    data: json!({"ok": true}),
                },
            ],
            ..Default::default()
        };

        let ir = MessageCardIr::from_plain(&card);
        assert_eq!(ir.head.title, Some("IR".into()));
        assert_eq!(ir.elements.len(), 2);
        assert_eq!(ir.actions.len(), 2);
        assert_eq!(ir.tier, Tier::Advanced);
    }
}
