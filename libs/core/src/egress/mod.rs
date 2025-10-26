use crate::prelude::*;
use serde_json::Value;

#[derive(Clone, Debug, Default)]
pub struct OutboundMessage {
    pub channel: Option<String>,
    pub text: Option<String>,
    pub payload: Option<Value>,
}

#[derive(Clone, Debug, Default)]
pub struct SendResult {
    pub message_id: Option<String>,
    pub raw: Option<Value>,
}

#[async_trait::async_trait]
pub trait EgressSender: Send + Sync {
    async fn send(&self, ctx: &TenantCtx, msg: OutboundMessage) -> NodeResult<SendResult>;
}
