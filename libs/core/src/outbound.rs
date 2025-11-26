use crate::{ChannelMessage, TenantCtx};
use serde::{Deserialize, Serialize};

/// Generic outbound envelope produced by flows/workers before channel-specific translation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutboundEnvelope {
    pub tenant: TenantCtx,
    pub channel_id: String,
    pub session_id: String,
    #[serde(default)]
    pub meta: serde_json::Value,
    #[serde(default)]
    pub body: serde_json::Value,
}

impl OutboundEnvelope {
    /// Convenience constructor from a ChannelMessage.
    pub fn for_channel(channel: &ChannelMessage, body: serde_json::Value) -> Self {
        Self {
            tenant: channel.tenant.clone(),
            channel_id: channel.channel_id.clone(),
            session_id: channel.session_id.clone(),
            meta: serde_json::Value::Null,
            body,
        }
    }
}
