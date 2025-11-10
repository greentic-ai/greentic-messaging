use anyhow::Result;
use async_trait::async_trait;
use dashmap::DashMap;

use crate::{ConversationScope, SessionKey, SessionRecord, SessionStore};

#[derive(Default)]
pub struct MemorySessionStore {
    by_session: DashMap<String, SessionRecord>,
    by_scope: DashMap<String, SessionKey>,
}

impl MemorySessionStore {
    pub fn new() -> Self {
        Self {
            by_session: DashMap::new(),
            by_scope: DashMap::new(),
        }
    }

    fn session_key(key: &SessionKey) -> String {
        key.as_str().to_string()
    }
}

#[async_trait]
impl SessionStore for MemorySessionStore {
    async fn save(&self, record: SessionRecord) -> Result<()> {
        let scope_key = record.scope.cache_key();
        let session_key = Self::session_key(&record.key);
        self.by_scope.insert(scope_key, record.key.clone());
        self.by_session.insert(session_key, record);
        Ok(())
    }

    async fn get(&self, key: &SessionKey) -> Result<Option<SessionRecord>> {
        Ok(self
            .by_session
            .get(&Self::session_key(key))
            .map(|entry| entry.value().clone()))
    }

    async fn find_by_scope(&self, scope: &ConversationScope) -> Result<Option<SessionRecord>> {
        if let Some(session_id) = self.by_scope.get(&scope.cache_key()) {
            let key = session_id.value().clone();
            return self.get(&key).await;
        }
        Ok(None)
    }

    async fn delete(&self, key: &SessionKey) -> Result<()> {
        if let Some(entry) = self.by_session.remove(&Self::session_key(key)) {
            let (_k, record) = entry;
            self.by_scope.remove(&record.scope.cache_key());
        }
        Ok(())
    }
}
