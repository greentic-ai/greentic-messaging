use anyhow::{Context, Result};
#[cfg(feature = "store_redis")]
use greentic_session::redis_store::RedisSessionStore;
use greentic_session::{SessionData, SessionKey, SessionStore, inmemory::InMemorySessionStore};
use greentic_types::{TenantCtx, UserId};
#[cfg(feature = "store_redis")]
use redis::Client;
use std::{env, sync::Arc};
use tokio::task;
#[cfg(not(feature = "store_redis"))]
use tracing::warn;

pub type DynSessionStore = dyn SessionStore + Send + Sync + 'static;

#[derive(Clone)]
pub struct SharedSessionStore {
    inner: Arc<DynSessionStore>,
}

impl SharedSessionStore {
    pub fn in_memory() -> Self {
        Self {
            inner: Arc::new(InMemorySessionStore::new()),
        }
    }

    #[cfg(feature = "store_redis")]
    fn with_redis(client: redis::Client, namespace: String) -> Self {
        Self {
            inner: Arc::new(RedisSessionStore::with_namespace(client, namespace)),
        }
    }

    pub async fn create_session(&self, ctx: TenantCtx, data: SessionData) -> Result<SessionKey> {
        self.spawn_blocking(move |store| store.create_session(&ctx, data))
            .await
    }

    pub async fn update_session(&self, key: SessionKey, data: SessionData) -> Result<()> {
        self.spawn_blocking(move |store| store.update_session(&key, data))
            .await
    }

    pub async fn find_by_user(
        &self,
        ctx: TenantCtx,
        user: UserId,
    ) -> Result<Option<(SessionKey, SessionData)>> {
        self.spawn_blocking(move |store| store.find_by_user(&ctx, &user))
            .await
    }

    async fn spawn_blocking<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(Arc<DynSessionStore>) -> greentic_types::GResult<R> + Send + 'static,
        R: Send + 'static,
    {
        let store = Arc::clone(&self.inner);
        task::spawn_blocking(move || f(store))
            .await
            .map_err(|err| anyhow::anyhow!("session task failed: {err}"))?
            .map_err(|err| anyhow::anyhow!("session store error: {err}"))
    }
}

pub async fn store_from_env() -> Result<SharedSessionStore> {
    match env::var("SESSION_REDIS_URL") {
        Ok(url) => {
            let namespace = env::var("SESSION_NAMESPACE").unwrap_or_else(|_| "gsm".into());
            redis_store(&url, namespace)
        }
        Err(_) => Ok(SharedSessionStore::in_memory()),
    }
}

#[cfg(feature = "store_redis")]
fn redis_store(url: &str, namespace: String) -> Result<SharedSessionStore> {
    let client =
        Client::open(url).with_context(|| format!("failed to open redis client for {url}"))?;
    Ok(SharedSessionStore::with_redis(client, namespace))
}

#[cfg(not(feature = "store_redis"))]
fn redis_store(url: &str, _namespace: String) -> Result<SharedSessionStore> {
    warn!(
        url = %url,
        "SESSION_REDIS_URL provided but store_redis feature disabled; falling back to in-memory store"
    );
    Ok(SharedSessionStore::in_memory())
}
