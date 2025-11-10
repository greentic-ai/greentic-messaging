use anyhow::Result;
use async_trait::async_trait;
use redis::AsyncCommands;
use tokio::sync::Mutex;

use crate::{ConversationScope, SessionKey, SessionRecord, SessionStore};

pub struct RedisSessionStore {
    namespace: String,
    connection: Mutex<redis::aio::ConnectionManager>,
}

impl RedisSessionStore {
    pub async fn connect(url: &str, namespace: impl Into<String>) -> Result<Self> {
        let client = redis::Client::open(url)?;
        let manager = redis::aio::ConnectionManager::new(client).await?;
        Ok(Self {
            namespace: namespace.into(),
            connection: Mutex::new(manager),
        })
    }

    fn session_key(&self, key: &SessionKey) -> String {
        format!("{}:session:{}", self.namespace, key.as_str())
    }

    fn conversation_key(&self, scope: &ConversationScope) -> String {
        format!("{}:conversation:{}", self.namespace, scope.cache_key())
    }
}

#[async_trait]
impl SessionStore for RedisSessionStore {
    async fn save(&self, record: SessionRecord) -> Result<()> {
        let payload = serde_json::to_string(&record)?;
        let session_key = self.session_key(&record.key);
        let conversation_key = self.conversation_key(&record.scope);
        let mut conn = self.connection.lock().await;
        redis::pipe()
            .cmd("SET")
            .arg(&session_key)
            .arg(payload)
            .ignore()
            .cmd("SET")
            .arg(&conversation_key)
            .arg(record.key.as_str())
            .ignore()
            .query_async::<()>(&mut *conn)
            .await?;
        Ok(())
    }

    async fn get(&self, key: &SessionKey) -> Result<Option<SessionRecord>> {
        let mut conn = self.connection.lock().await;
        let session_key = self.session_key(key);
        let payload: Option<String> = conn.get(session_key).await?;
        let record = if let Some(raw) = payload {
            Some(serde_json::from_str(&raw)?)
        } else {
            None
        };
        Ok(record)
    }

    async fn find_by_scope(&self, scope: &ConversationScope) -> Result<Option<SessionRecord>> {
        let mut conn = self.connection.lock().await;
        let conversation_key = self.conversation_key(scope);
        let session_id: Option<String> = conn.get(conversation_key).await?;
        if let Some(id) = session_id {
            return self.get(&SessionKey::new(id)).await;
        }
        Ok(None)
    }

    async fn delete(&self, key: &SessionKey) -> Result<()> {
        let record = self.get(key).await?;
        if let Some(rec) = record {
            let session_key = self.session_key(&rec.key);
            let conversation_key = self.conversation_key(&rec.scope);
            let mut conn = self.connection.lock().await;
            redis::pipe()
                .cmd("DEL")
                .arg(&session_key)
                .ignore()
                .cmd("DEL")
                .arg(&conversation_key)
                .ignore()
                .query_async::<()>(&mut *conn)
                .await?;
        }
        Ok(())
    }
}
