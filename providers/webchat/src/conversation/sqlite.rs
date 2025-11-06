use std::{
    collections::{HashMap, VecDeque},
    path::Path,
    sync::Arc,
};

use rusqlite::{params, Connection};
use tokio::{
    sync::{broadcast, Mutex},
    task::spawn_blocking,
};

use super::{
    Activity, ActivityPage, ConversationStore, SharedConversationStore, StoreError, StoredActivity,
    MAX_ACTIVITY_HISTORY,
};
use greentic_types::TenantCtx;

const CREATE_TABLE_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS conversations (
    id TEXT PRIMARY KEY,
    tenant_ctx TEXT NOT NULL,
    activities TEXT NOT NULL,
    next_watermark INTEGER NOT NULL
);
"#;

#[derive(Clone)]
pub struct SqliteConversationStore {
    conn: Arc<Mutex<Connection>>,
    channels: Arc<Mutex<HashMap<String, broadcast::Sender<StoredActivity>>>>,
}

pub fn sqlite_store(path: impl AsRef<Path>) -> anyhow::Result<SharedConversationStore> {
    let conn = Connection::open(path)?;
    conn.execute_batch(CREATE_TABLE_SQL)?;
    Ok(Arc::new(SqliteConversationStore {
        conn: Arc::new(Mutex::new(conn)),
        channels: Arc::new(Mutex::new(HashMap::new())),
    }))
}

#[derive(serde::Serialize, serde::Deserialize)]
struct PersistedRecord {
    ctx: TenantCtx,
    activities: Vec<StoredActivity>,
    next_watermark: u64,
}

impl SqliteConversationStore {
    async fn with_conn<F, T>(&self, func: F) -> Result<T, StoreError>
    where
        F: FnOnce(&Connection) -> Result<T, StoreError> + Send + 'static,
        T: Send + 'static,
    {
        let conn = Arc::clone(&self.conn);
        spawn_blocking(move || {
            let guard = conn.lock().unwrap();
            func(&guard)
        })
        .await
        .map_err(|err| StoreError::Internal(err.into()))?
    }

    async fn load_record(&self, conversation_id: &str) -> Result<Option<PersistedRecord>, StoreError>
    where
    {
        let id = conversation_id.to_string();
        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT tenant_ctx, activities, next_watermark FROM conversations WHERE id = ?1",
                )
                .map_err(|err| StoreError::Internal(err.into()))?;
            let mut rows = stmt
                .query(params![id])
                .map_err(|err| StoreError::Internal(err.into()))?;
            if let Some(row) = rows
                .next()
                .map_err(|err| StoreError::Internal(err.into()))?
            {
                let ctx: String = row
                    .get(0)
                    .map_err(|err| StoreError::Internal(err.into()))?;
                let activities_json: String = row
                    .get(1)
                    .map_err(|err| StoreError::Internal(err.into()))?;
                let next_watermark: u64 = row
                    .get(2)
                    .map_err(|err| StoreError::Internal(err.into()))?;
                let activities: Vec<StoredActivity> = serde_json::from_str(&activities_json)
                    .map_err(|err| StoreError::Internal(err.into()))?;
                let ctx: TenantCtx =
                    serde_json::from_str(&ctx).map_err(|err| StoreError::Internal(err.into()))?;
                Ok(Some(PersistedRecord {
                    ctx,
                    activities,
                    next_watermark,
                }))
            } else {
                Ok(None)
            }
        })
        .await
    }

    async fn save_record(
        &self,
        conversation_id: &str,
        record: &PersistedRecord,
    ) -> Result<(), StoreError> {
        let id = conversation_id.to_string();
        let ctx_json =
            serde_json::to_string(&record.ctx).map_err(|err| StoreError::Internal(err.into()))?;
        let activities =
            serde_json::to_string(&record.activities).map_err(|err| StoreError::Internal(err.into()))?;
        let next_watermark = record.next_watermark;

        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO conversations (id, tenant_ctx, activities, next_watermark)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(id) DO UPDATE SET tenant_ctx=excluded.tenant_ctx,
                 activities=excluded.activities,
                 next_watermark=excluded.next_watermark",
                params![id, ctx_json, activities, next_watermark],
            )
            .map_err(|err| StoreError::Internal(err.into()))?;
            Ok(())
        })
        .await
    }
}

#[async_trait::async_trait]
impl ConversationStore for SqliteConversationStore {
    async fn create(&self, conversation_id: &str, ctx: TenantCtx) -> Result<(), StoreError> {
        {
            if self
                .load_record(conversation_id)
                .await?
                .is_some()
            {
                return Err(StoreError::AlreadyExists(conversation_id.to_string()));
            }
        }
        let record = PersistedRecord {
            ctx,
            activities: Vec::new(),
            next_watermark: 0,
        };
        self.save_record(conversation_id, &record).await?;
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
        let mut record = self
            .load_record(conversation_id)
            .await?
            .ok_or_else(|| StoreError::NotFound(conversation_id.to_string()))?;

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
        self.save_record(conversation_id, &record).await?;

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
        let record = self
            .load_record(conversation_id)
            .await?
            .ok_or_else(|| StoreError::NotFound(conversation_id.to_string()))?;
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
        let record = self
            .load_record(conversation_id)
            .await?
            .ok_or_else(|| StoreError::NotFound(conversation_id.to_string()))?;
        Ok(record.ctx)
    }

    async fn subscribe(
        &self,
        conversation_id: &str,
    ) -> Result<broadcast::Receiver<StoredActivity>, StoreError> {
        let channels = self.channels.lock().await;
        let sender = channels
            .get(conversation_id)
            .ok_or_else(|| StoreError::NotFound(conversation_id.to_string()))?
            .clone();
        Ok(sender.subscribe())
    }
}
