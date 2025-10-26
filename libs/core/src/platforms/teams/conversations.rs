use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Conversation reference used to deliver proactive Teams messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamsConversation {
    pub chat_id: String,
    #[serde(default)]
    pub conversation_id: Option<String>,
    #[serde(default)]
    pub service_url: Option<String>,
}

impl TeamsConversation {
    pub fn new(chat_id: impl Into<String>) -> Self {
        Self {
            chat_id: chat_id.into(),
            conversation_id: None,
            service_url: None,
        }
    }
}

/// Collection of conversations keyed by logical channel identifier.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TeamsConversations {
    #[serde(default)]
    pub items: HashMap<String, TeamsConversation>,
}

impl TeamsConversations {
    pub fn get(&self, channel: &str) -> Option<&TeamsConversation> {
        self.items.get(channel)
    }

    pub fn insert(&mut self, channel: impl Into<String>, conversation: TeamsConversation) {
        self.items.insert(channel.into(), conversation);
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}
