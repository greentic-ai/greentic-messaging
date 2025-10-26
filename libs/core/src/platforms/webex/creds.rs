use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WebexWebhook {
    pub id: String,
    pub resource: String,
    pub event: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebexCredentials {
    pub bot_token: String,
    pub webhook_secret: String,
    #[serde(default)]
    pub webhooks: Vec<WebexWebhook>,
}

impl WebexCredentials {
    pub fn has_subscription(&self, resource: &str, event: &str) -> bool {
        self.webhooks
            .iter()
            .any(|hook| hook.resource == resource && hook.event == event)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebexCreds {
    pub bot_token: String,
}
