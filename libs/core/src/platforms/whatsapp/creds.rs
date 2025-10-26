use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WhatsAppCredentials {
    pub phone_id: String,
    pub wa_user_token: String,
    pub app_secret: String,
    pub verify_token: String,
    #[serde(default)]
    pub webhook_subscribed: bool,
    #[serde(default)]
    pub subscription_signature: Option<String>,
}

impl WhatsAppCredentials {
    pub fn fingerprint(&self) -> String {
        format!(
            "{}:{}:{}",
            self.phone_id, self.wa_user_token, self.verify_token
        )
    }
}
