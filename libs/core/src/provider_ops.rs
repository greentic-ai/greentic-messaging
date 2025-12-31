//! Provider-core messaging operation contracts (schemas + serde types).
//!
//! These types intentionally mirror the JSON Schemas under `schemas/messaging/ops`.

use greentic_types::{ChannelMessageEnvelope, TenantCtx};
#[cfg(feature = "schemars")]
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Canonical message envelope used by ingest/output operations.
pub type MessageEnvelope = ChannelMessageEnvelope;

/// Output shape for ingest; aliasing the canonical envelope keeps schema parity.
pub type IngestOutput = ChannelMessageEnvelope;

/// Attachment payload embedded in send/reply requests.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct AttachmentInput {
    pub name: String,
    pub content_type: String,
    pub data_base64: String,
}

/// Optional routing hints for sends.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct SendMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

/// Input contract for the send operation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct SendInput {
    pub to: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<AttachmentInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<SendMetadata>,
}

/// Input contract for a reply operation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct ReplyInput {
    pub to: String,
    pub reply_to: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<AttachmentInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<ReplyMetadata>,
}

/// Metadata attached to replies.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct ReplyMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

/// Delivery status reported by providers.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub enum SendStatus {
    Sent,
    Queued,
}

/// Output contract shared by send/reply.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct SendOutput {
    pub message_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    pub status: SendStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ts: Option<String>,
}

/// Output contract for replies; shares the same fields as send.
pub type ReplyOutput = SendOutput;

/// Input contract for ingest (webhook normalization).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct IngestInput {
    pub provider_type: String,
    pub payload: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tenant: Option<TenantCtx>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub received_at: Option<String>,
}
