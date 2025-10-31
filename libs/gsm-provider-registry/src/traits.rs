use async_trait::async_trait;
use gsm_core::TenantCtx;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::errors::MsgError;

/// Canonical cross-provider message representation.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Message {
    #[serde(default)]
    pub to: Option<String>,
    #[serde(default)]
    pub chat_id: Option<String>,
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub thread_id: Option<String>,
    #[serde(default)]
    pub metadata: Value,
}

/// Result returned by a `SendAdapter`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SendResult {
    pub provider_message_id: String,
    pub delivered: bool,
    #[serde(default)]
    pub raw: Value,
}

#[async_trait]
pub trait SendAdapter: Send + Sync {
    async fn send(&self, ctx: &TenantCtx, message: &Message) -> Result<SendResult, MsgError>;
}

pub trait ReceiveAdapter: Send + Sync {
    fn ingest(&self, ctx: &TenantCtx, payload: Value) -> Result<Vec<Message>, MsgError>;
}
