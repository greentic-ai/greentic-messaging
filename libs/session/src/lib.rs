mod memory;
#[cfg(feature = "redis-store")]
mod redis_store;

use std::{env, sync::Arc};

use anyhow::Result;
use async_trait::async_trait;
use greentic_types::{TenantCtx, session::SessionKey};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
#[cfg(not(feature = "redis-store"))]
use tracing::warn;

pub use memory::MemorySessionStore;
#[cfg(feature = "redis-store")]
pub use redis_store::RedisSessionStore;

/// Shared session store handle used across services.
pub type SharedSessionStore = Arc<dyn SessionStore>;

/// Scope describing a user conversation on a channel.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ConversationScope {
    pub env: String,
    pub tenant: String,
    pub platform: String,
    pub chat_id: String,
    pub user_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
}

impl ConversationScope {
    pub fn new(
        env: impl Into<String>,
        tenant: impl Into<String>,
        platform: impl Into<String>,
        chat_id: impl Into<String>,
        user_id: impl Into<String>,
        thread_id: Option<String>,
    ) -> Self {
        Self {
            env: env.into(),
            tenant: tenant.into(),
            platform: platform.into(),
            chat_id: chat_id.into(),
            user_id: user_id.into(),
            thread_id,
        }
    }

    pub fn cache_key(&self) -> String {
        format!(
            "{}:{}:{}:{}:{}:{}",
            self.env,
            self.tenant,
            self.platform,
            self.chat_id,
            self.user_id,
            self.thread_id.as_deref().unwrap_or_default()
        )
    }
}

/// Serialized snapshot captured for a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSnapshot {
    pub tenant_ctx: TenantCtx,
    pub flow_id: String,
    pub cursor_node: String,
    pub context_json: String,
}

/// Persisted record tying a session identifier with its flow snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRecord {
    pub key: SessionKey,
    pub scope: ConversationScope,
    pub snapshot: SessionSnapshot,
    pub updated_unix_ms: i128,
}

impl SessionRecord {
    pub fn new(key: SessionKey, scope: ConversationScope, snapshot: SessionSnapshot) -> Self {
        Self {
            key,
            scope,
            snapshot,
            updated_unix_ms: OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000,
        }
    }
}

#[async_trait]
pub trait SessionStore: Send + Sync {
    async fn save(&self, record: SessionRecord) -> Result<()>;
    async fn get(&self, key: &SessionKey) -> Result<Option<SessionRecord>>;
    async fn find_by_scope(&self, scope: &ConversationScope) -> Result<Option<SessionRecord>>;
    async fn delete(&self, key: &SessionKey) -> Result<()>;
}

/// Returns an in-memory session store wrapped in an [`Arc`].
pub fn shared_memory_store() -> SharedSessionStore {
    Arc::new(MemorySessionStore::new())
}

/// Builds a session store from environment variables.
///
/// If `SESSION_REDIS_URL` is present and the `redis-store` feature is enabled, a Redis-backed
/// store is created. Otherwise, the function falls back to the in-memory implementation.
pub async fn store_from_env() -> Result<SharedSessionStore> {
    match env::var("SESSION_REDIS_URL") {
        Ok(url) => {
            let namespace = env::var("SESSION_NAMESPACE").unwrap_or_else(|_| "gsm".into());
            build_redis_store(&url, &namespace).await
        }
        Err(_) => Ok(shared_memory_store()),
    }
}

#[cfg(feature = "redis-store")]
async fn build_redis_store(url: &str, namespace: &str) -> Result<SharedSessionStore> {
    let store = RedisSessionStore::connect(url, namespace).await?;
    Ok(Arc::new(store))
}

#[cfg(not(feature = "redis-store"))]
async fn build_redis_store(_url: &str, _namespace: &str) -> Result<SharedSessionStore> {
    warn!("redis-store feature disabled; using in-memory session store");
    Ok(shared_memory_store())
}
