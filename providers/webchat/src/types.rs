use std::collections::BTreeMap;

use greentic_types::TenantCtx;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GreenticEvent {
    IncomingMessage(IncomingMessage),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct IncomingMessage {
    pub id: String,
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    pub tenant_ctx: TenantCtx,
    pub conversation: ConversationRef,
    pub from: Participant,
    pub payload: MessagePayload,
    #[serde(default)]
    pub channel_data: BTreeMap<String, Value>,
    pub raw_activity: Value,
}

impl IncomingMessage {
    pub fn new(id: Option<String>, tenant_ctx: TenantCtx) -> Self {
        Self {
            id: id.unwrap_or_else(|| Uuid::new_v4().to_string()),
            at: OffsetDateTime::now_utc(),
            tenant_ctx,
            conversation: ConversationRef {
                channel: "webchat".into(),
                conversation_id: String::new(),
            },
            from: Participant {
                id: String::new(),
                name: None,
                role: None,
            },
            payload: MessagePayload::Typing,
            channel_data: BTreeMap::new(),
            raw_activity: Value::Null,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ConversationRef {
    pub channel: String,
    pub conversation_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Participant {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MessagePayload {
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        locale: Option<String>,
    },
    Typing,
    Event {
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        value: Option<Value>,
    },
    Attachment {
        content_type: String,
        content: Value,
    },
}
