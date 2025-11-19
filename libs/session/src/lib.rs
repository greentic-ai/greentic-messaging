use anyhow::{Context, Result};
pub use greentic_session::{SessionData, SessionKey};
use greentic_session::{
    SessionStore, inmemory::InMemorySessionStore, redis_store::RedisSessionStore,
};
use greentic_types::{TenantCtx, UserId};
use redis::Client;
use std::sync::Arc;
use tokio::task;

const DEFAULT_NAMESPACE: &str = "greentic:session";
const SESSION_NAMESPACE_ENV: &str = "SESSION_NAMESPACE";
const SESSION_REDIS_URL_ENV: &str = "SESSION_REDIS_URL";

/// Shared session store handle that wraps the greentic-session backends.
#[derive(Clone)]
pub struct SharedSessionStore {
    inner: Arc<SessionBackend>,
}

enum SessionBackend {
    InMemory(Arc<InMemorySessionStore>),
    Redis(Arc<RedisSessionStore>),
}

/// Builds a session store from environment configuration.
pub async fn store_from_env() -> Result<SharedSessionStore> {
    match std::env::var(SESSION_REDIS_URL_ENV) {
        Ok(url) => {
            let namespace =
                std::env::var(SESSION_NAMESPACE_ENV).unwrap_or_else(|_| DEFAULT_NAMESPACE.into());
            build_redis_store(&url, &namespace)
        }
        Err(_) => Ok(shared_memory_store()),
    }
}

/// Returns an in-memory session store.
pub fn shared_memory_store() -> SharedSessionStore {
    SharedSessionStore {
        inner: Arc::new(SessionBackend::InMemory(Arc::new(
            InMemorySessionStore::new(),
        ))),
    }
}

fn build_redis_store(url: &str, namespace: &str) -> Result<SharedSessionStore> {
    let client = Client::open(url).context("invalid SESSION_REDIS_URL")?;
    let store = RedisSessionStore::with_namespace(client, namespace);
    Ok(SharedSessionStore {
        inner: Arc::new(SessionBackend::Redis(Arc::new(store))),
    })
}

impl SharedSessionStore {
    /// Looks up the active session bound to the provided tenant + user combination.
    pub async fn find_by_user(
        &self,
        ctx: &TenantCtx,
        user: &UserId,
    ) -> Result<Option<(SessionKey, SessionData)>> {
        match self.inner.as_ref() {
            SessionBackend::InMemory(store) => store.find_by_user(ctx, user).map_err(Into::into),
            SessionBackend::Redis(store) => {
                let store = Arc::clone(store);
                let ctx = ctx.clone();
                let user = user.clone();
                blocking_call(move || store.find_by_user(&ctx, &user)).await
            }
        }
    }

    /// Creates a new session and returns its key.
    pub async fn create_session(&self, ctx: &TenantCtx, data: SessionData) -> Result<SessionKey> {
        match self.inner.as_ref() {
            SessionBackend::InMemory(store) => store.create_session(ctx, data).map_err(Into::into),
            SessionBackend::Redis(store) => {
                let store = Arc::clone(store);
                let ctx = ctx.clone();
                blocking_call(move || store.create_session(&ctx, data)).await
            }
        }
    }

    /// Updates an existing session with the supplied snapshot.
    pub async fn update_session(&self, key: &SessionKey, data: SessionData) -> Result<()> {
        match self.inner.as_ref() {
            SessionBackend::InMemory(store) => store.update_session(key, data).map_err(Into::into),
            SessionBackend::Redis(store) => {
                let store = Arc::clone(store);
                let key = key.clone();
                blocking_call(move || store.update_session(&key, data)).await
            }
        }
    }
}

async fn blocking_call<T, F>(f: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> greentic_types::GResult<T> + Send + 'static,
{
    task::spawn_blocking(move || f().map_err(anyhow::Error::from))
        .await
        .context("session store operation failed")?
}
