use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
};

use anyhow::Error;
use greentic_types::TenantCtx;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::OffsetDateTime;
use tokio::sync::{RwLock, broadcast};
use uuid::Uuid;

/// Bot Framework activity representation stored by the Direct Line server.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Activity {
    #[serde(default)]
    pub id: String,
    pub r#type: String,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub timestamp: Option<OffsetDateTime>,
    #[serde(default)]
    pub from: Option<ChannelAccount>,
    #[serde(default)]
    pub recipient: Option<ChannelAccount>,
    #[serde(default)]
    pub conversation: Option<ConversationAccount>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    #[serde(default)]
    pub channel_data: Option<serde_json::Value>,
    #[serde(default)]
    pub value: Option<serde_json::Value>,
    #[serde(default)]
    pub locale: Option<String>,
    #[serde(default)]
    pub reply_to_id: Option<String>,
    #[serde(default)]
    pub entities: Vec<serde_json::Value>,
    #[serde(default)]
    pub service_url: Option<String>,
    #[serde(default)]
    pub channel_id: Option<String>,
    #[serde(default)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl Activity {
    /// Creates a new activity with the provided type and empty payload.
    pub fn new(r#type: impl Into<String>) -> Self {
        Self {
            id: String::new(),
            r#type: r#type.into(),
            timestamp: None,
            from: None,
            recipient: None,
            conversation: None,
            text: None,
            attachments: Vec::new(),
            channel_data: None,
            value: None,
            locale: None,
            reply_to_id: None,
            entities: Vec::new(),
            service_url: None,
            channel_id: None,
            extra: serde_json::Map::new(),
        }
    }

    fn ensure_defaults(&mut self, conversation_id: &str) {
        if self.id.trim().is_empty() {
            self.id = Uuid::new_v4().to_string();
        }
        if self.r#type.trim().is_empty() {
            self.r#type = "message".into();
        }
        if self.timestamp.is_none() {
            self.timestamp = Some(OffsetDateTime::now_utc());
        }
        if self
            .conversation
            .as_ref()
            .map(|c| c.id.trim().is_empty())
            .unwrap_or(true)
        {
            self.conversation = Some(ConversationAccount {
                id: conversation_id.to_string(),
            });
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ChannelAccount {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ConversationAccount {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    pub content_type: String,
    #[serde(default)]
    pub content: serde_json::Value,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub thumbnail_url: Option<String>,
}

/// Stored activity with its associated watermark.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredActivity {
    pub watermark: u64,
    pub activity: Activity,
}

/// Batch of activities returned from the store.
#[derive(Debug, Clone, PartialEq)]
pub struct ActivityPage {
    pub activities: VecDeque<StoredActivity>,
    pub watermark: u64,
}

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("conversation already exists: {0}")]
    AlreadyExists(String),
    #[error("conversation not found: {0}")]
    NotFound(String),
    #[error("conversation quota exceeded: {0}")]
    QuotaExceeded(String),
    #[error("conversation store internal error: {0}")]
    Internal(#[from] Error),
}

pub type Result<T> = std::result::Result<T, StoreError>;

#[async_trait::async_trait]
pub trait ConversationStore: Send + Sync {
    async fn create(&self, conversation_id: &str, ctx: TenantCtx) -> Result<()>;
    async fn append(&self, conversation_id: &str, activity: Activity) -> Result<StoredActivity>;
    async fn activities(
        &self,
        conversation_id: &str,
        watermark: Option<u64>,
    ) -> Result<ActivityPage>;
    async fn tenant_ctx(&self, conversation_id: &str) -> Result<TenantCtx>;
    async fn subscribe(&self, conversation_id: &str)
    -> Result<broadcast::Receiver<StoredActivity>>;
}

pub type SharedConversationStore = Arc<dyn ConversationStore>;

struct NoopStore;

#[async_trait::async_trait]
impl ConversationStore for NoopStore {
    async fn create(&self, conversation_id: &str, _ctx: TenantCtx) -> Result<()> {
        Err(StoreError::NotFound(conversation_id.to_string()))
    }

    async fn append(&self, conversation_id: &str, _activity: Activity) -> Result<StoredActivity> {
        Err(StoreError::NotFound(conversation_id.to_string()))
    }

    async fn activities(
        &self,
        conversation_id: &str,
        _watermark: Option<u64>,
    ) -> Result<ActivityPage> {
        Err(StoreError::NotFound(conversation_id.to_string()))
    }

    async fn tenant_ctx(&self, conversation_id: &str) -> Result<TenantCtx> {
        Err(StoreError::NotFound(conversation_id.to_string()))
    }

    async fn subscribe(
        &self,
        conversation_id: &str,
    ) -> Result<broadcast::Receiver<StoredActivity>> {
        Err(StoreError::NotFound(conversation_id.to_string()))
    }
}

pub fn noop_store() -> SharedConversationStore {
    Arc::new(NoopStore)
}

/// In-memory implementation backed by a `tokio::sync::RwLock`.
pub struct InMemoryConversationStore {
    inner: RwLock<HashMap<String, ConversationRecord>>,
}

pub(super) const MAX_ACTIVITY_HISTORY: usize = 5_000;

struct ConversationRecord {
    ctx: TenantCtx,
    activities: VecDeque<StoredActivity>,
    next_watermark: u64,
    broadcaster: broadcast::Sender<StoredActivity>,
}

impl InMemoryConversationStore {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(HashMap::new()),
        }
    }

    pub fn shared() -> SharedConversationStore {
        Arc::new(Self::new())
    }
}

impl Default for InMemoryConversationStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl ConversationStore for InMemoryConversationStore {
    async fn create(&self, conversation_id: &str, ctx: TenantCtx) -> Result<()> {
        let mut guard = self.inner.write().await;
        if guard.contains_key(conversation_id) {
            return Err(StoreError::AlreadyExists(conversation_id.to_string()));
        }
        let (tx, _rx) = broadcast::channel(32);
        guard.insert(
            conversation_id.to_string(),
            ConversationRecord {
                ctx,
                activities: VecDeque::new(),
                next_watermark: 0,
                broadcaster: tx,
            },
        );
        Ok(())
    }

    async fn append(
        &self,
        conversation_id: &str,
        mut activity: Activity,
    ) -> Result<StoredActivity> {
        let mut guard = self.inner.write().await;
        let record = guard
            .get_mut(conversation_id)
            .ok_or_else(|| StoreError::NotFound(conversation_id.to_string()))?;

        if record.activities.len() >= MAX_ACTIVITY_HISTORY {
            return Err(StoreError::QuotaExceeded(conversation_id.to_string()));
        }

        activity.ensure_defaults(conversation_id);
        let watermark = record.next_watermark;
        record.next_watermark = record.next_watermark.saturating_add(1);
        let stored = StoredActivity {
            watermark,
            activity: activity.clone(),
        };
        record.activities.push_back(stored.clone());
        let sender = record.broadcaster.clone();
        drop(guard);
        let _ = sender.send(stored.clone());
        Ok(stored)
    }

    async fn activities(
        &self,
        conversation_id: &str,
        watermark: Option<u64>,
    ) -> Result<ActivityPage> {
        let guard = self.inner.read().await;
        let record = guard
            .get(conversation_id)
            .ok_or_else(|| StoreError::NotFound(conversation_id.to_string()))?;
        let start = watermark.unwrap_or(0) as usize;
        if start > record.next_watermark as usize {
            return Ok(ActivityPage {
                activities: VecDeque::new(),
                watermark: record.next_watermark,
            });
        }
        let slice = record
            .activities
            .iter()
            .skip(start)
            .cloned()
            .collect::<VecDeque<_>>();
        Ok(ActivityPage {
            activities: slice,
            watermark: record.next_watermark,
        })
    }

    async fn tenant_ctx(&self, conversation_id: &str) -> Result<TenantCtx> {
        let guard = self.inner.read().await;
        let record = guard
            .get(conversation_id)
            .ok_or_else(|| StoreError::NotFound(conversation_id.to_string()))?;
        Ok(record.ctx.clone())
    }

    async fn subscribe(
        &self,
        conversation_id: &str,
    ) -> Result<broadcast::Receiver<StoredActivity>> {
        let guard = self.inner.read().await;
        let record = guard
            .get(conversation_id)
            .ok_or_else(|| StoreError::NotFound(conversation_id.to_string()))?;
        Ok(record.broadcaster.subscribe())
    }
}

/// Creates a shared in-memory conversation store.
pub fn memory_store() -> SharedConversationStore {
    InMemoryConversationStore::shared()
}

#[cfg(feature = "store_sqlite")]
pub mod sqlite;
#[cfg(feature = "store_sqlite")]
pub use sqlite::sqlite_store;

#[cfg(feature = "store_redis")]
pub mod redis;
#[cfg(feature = "store_redis")]
pub use redis::redis_store;

#[cfg(test)]
mod tests {
    use super::*;

    use greentic_types::{EnvId, TeamId, TenantId};

    fn ctx() -> TenantCtx {
        TenantCtx::new(EnvId::from("dev"), TenantId::from("acme"))
            .with_team(Some(TeamId::from("support")))
    }

    #[tokio::test]
    async fn append_and_read_watermarks() {
        let store = InMemoryConversationStore::new();
        store.create("conv-1", ctx()).await.unwrap();

        let mut msg = Activity::new("message");
        msg.text = Some("hello".into());
        let stored = store.append("conv-1", msg).await.unwrap();
        assert_eq!(stored.watermark, 0);
        assert!(!stored.activity.id.is_empty());

        let page = store.activities("conv-1", None).await.unwrap();
        assert_eq!(page.activities.len(), 1);
        assert_eq!(page.watermark, 1);

        let page_empty = store
            .activities("conv-1", Some(page.watermark))
            .await
            .unwrap();
        assert!(page_empty.activities.is_empty());

        let stored2 = store
            .append("conv-1", Activity::new("event"))
            .await
            .unwrap();
        assert_eq!(stored2.watermark, 1);
        let page_delta = store.activities("conv-1", Some(1)).await.unwrap();
        assert_eq!(page_delta.activities.len(), 1);
        assert_eq!(page_delta.activities[0].watermark, 1);
    }

    #[tokio::test]
    async fn subscriber_receives_new_activities() {
        let store = InMemoryConversationStore::new();
        store.create("conv-2", ctx()).await.unwrap();
        let mut subscriber = store.subscribe("conv-2").await.unwrap();

        store
            .append("conv-2", Activity::new("message"))
            .await
            .unwrap();

        let received = subscriber.recv().await.unwrap();
        assert_eq!(received.watermark, 0);
    }

    #[tokio::test]
    async fn tenant_ctx_round_trip() {
        let store = InMemoryConversationStore::new();
        let context = ctx();
        store.create("conv-3", context.clone()).await.unwrap();
        let fetched = store.tenant_ctx("conv-3").await.unwrap();
        assert_eq!(fetched, context);
    }
}
