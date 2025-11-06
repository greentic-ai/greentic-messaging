use anyhow::Result;
use serde_json::{Value, json};

use crate::types::{CardBlock, MessageCard};

pub type Card = MessageCard;

pub trait CardRenderer: Send + Sync {
    fn render(&self, card: &Card) -> Result<Value>;
}

impl Card {
    pub fn from_text(text: &str) -> Self {
        Self {
            title: None,
            body: vec![CardBlock::Text {
                text: text.to_string(),
                markdown: false,
            }],
            actions: Vec::new(),
        }
    }

    pub fn into_json(self) -> serde_json::Value {
        json!(self)
    }
}
