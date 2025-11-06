use std::{collections::VecDeque, sync::Arc};

use redis::{AsyncCommands, aio::ConnectionManager};
use tokio::sync::{Mutex, broadcast};

use super::{
    Activity, ActivityPage, ConversationStore, MAX_ACTIVITY_HISTORY, SharedConversationStore,
    StoreError, StoredActivity,
};
use greentic_types::TenantCtx;

#[derive(Clone)]
pub struct RedisConversationStore {
    client: redis::Client,
    channels: Arc<Mutex<std::collections::HashMap<String, broadcast::Sender<StoredActivity>>>>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct PersistedRecord {
    ctx: TenantCtx,
    activities: Vec<StoredActivity>,
    next_watermark: u64,
}

pub async fn redis_store(connection_string: &str) -> anyhow::Result<SharedConversationStore> {
    let client = redis::Client::open(connection_string)?;
    Ok(Arc::new(RedisConversationStore {
        client,
        channels: Arc::new(Mutex::new(std::collections::HashMap::new())),
    }))
}

impl RedisConversationStore {
    async fn connection(&self) -> Result<ConnectionManager, StoreError> {
        ConnectionManager::new(self.client.clone())
            .await
            .map_err(|err| StoreError::Internal(err.into()))
    }

    fn key(&self, conversation_id: &str) -> String {
        format!("webchat:conversation:{conversation_id}")
    }
}

#[async_trait::async_trait]
impl ConversationStore for RedisConversationStore {
    async fn create(&self, conversation_id: &str, ctx: TenantCtx) -> Result<(), StoreError> {
        let mut conn = self.connection().await?;
        let key = self.key(conversation_id);
        let exists: bool = conn
            .exists(&key)
            .await
            .map_err(|err| StoreError::Internal(err.into()))?;
        if exists {
            return Err(StoreError::AlreadyExists(conversation_id.to_string()));
        }
        let record = PersistedRecord {
            ctx,
            activities: Vec::new(),
            next_watermark: 0,
        };
        let payload =
            serde_json::to_string(&record).map_err(|err| StoreError::Internal(err.into()))?;
        conn.set::<_, _, ()>(&key, payload)
            .await
            .map_err(|err| StoreError::Internal(err.into()))?;
        self.channels
            .lock()
            .await
            .insert(conversation_id.to_string(), broadcast::channel(32).0);
        Ok(())
    }

    async fn append(
        &self,
        conversation_id: &str,
        mut activity: Activity,
    ) -> Result<StoredActivity, StoreError> {
        let key = self.key(conversation_id);
        let mut conn = self.connection().await?;
        let payload: Option<String> = conn
            .get(&key)
            .await
            .map_err(|err| StoreError::Internal(err.into()))?;
        let mut record = match payload {
            Some(data) => serde_json::from_str::<PersistedRecord>(&data)
                .map_err(|err| StoreError::Internal(err.into()))?,
            None => return Err(StoreError::NotFound(conversation_id.to_string())),
        };

        if record.activities.len() >= MAX_ACTIVITY_HISTORY {
            return Err(StoreError::QuotaExceeded(conversation_id.to_string()));
        }

        activity.ensure_defaults(conversation_id);
        let stored = StoredActivity {
            watermark: record.next_watermark,
            activity: activity.clone(),
        };
        record.activities.push(stored.clone());
        record.next_watermark = record.next_watermark.saturating_add(1);
        let payload =
            serde_json::to_string(&record).map_err(|err| StoreError::Internal(err.into()))?;
        conn.set::<_, _, ()>(&key, payload)
            .await
            .map_err(|err| StoreError::Internal(err.into()))?;

        if let Some(sender) = self.channels.lock().await.get(conversation_id) {
            let _ = sender.send(stored.clone());
        }

        Ok(stored)
    }

    async fn activities(
        &self,
        conversation_id: &str,
        watermark: Option<u64>,
    ) -> Result<ActivityPage, StoreError> {
        let key = self.key(conversation_id);
        let mut conn = self.connection().await?;
        let payload: Option<String> = conn
            .get(&key)
            .await
            .map_err(|err| StoreError::Internal(err.into()))?;
        let record = match payload {
            Some(data) => serde_json::from_str::<PersistedRecord>(&data)
                .map_err(|err| StoreError::Internal(err.into()))?,
            None => return Err(StoreError::NotFound(conversation_id.to_string())),
        };

        let start = watermark.unwrap_or(0) as usize;
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

    async fn tenant_ctx(&self, conversation_id: &str) -> Result<TenantCtx, StoreError> {
        let key = self.key(conversation_id);
        let mut conn = self.connection().await?;
        let payload: Option<String> = conn
            .get(&key)
            .await
            .map_err(|err| StoreError::Internal(err.into()))?;
        match payload {
            Some(data) => {
                let record: PersistedRecord =
                    serde_json::from_str(&data).map_err(|err| StoreError::Internal(err.into()))?;
                Ok(record.ctx)
            }
            None => Err(StoreError::NotFound(conversation_id.to_string())),
        }
    }

    async fn subscribe(
        &self,
        conversation_id: &str,
    ) -> Result<broadcast::Receiver<StoredActivity>, StoreError> {
        let mut guard = self.channels.lock().await;
        if !guard.contains_key(conversation_id) {
            return Err(StoreError::NotFound(conversation_id.to_string()));
        }
        let sender = guard
            .entry(conversation_id.to_string())
            .or_insert_with(|| broadcast::channel(32).0);
        Ok(sender.subscribe())
    }
}
