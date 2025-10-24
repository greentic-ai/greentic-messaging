use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TelegramCreds {
    pub bot_token: String,
    pub webhook_secret: String,
    #[serde(default)]
    pub webhook_set: bool,
}
