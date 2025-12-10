use anyhow::Result;
pub use greentic_session::{SessionData, SessionKey};
use greentic_session::{SessionStore, inmemory::InMemorySessionStore};
use greentic_types::{TenantCtx, UserId};
use std::sync::Arc;

/// Shared session store handle that wraps the greentic-session backends.
#[derive(Clone)]
pub struct SharedSessionStore {
    inner: Arc<InMemorySessionStore>,
}

/// Builds a session store from environment configuration.
pub async fn store_from_env() -> Result<SharedSessionStore> {
    Ok(shared_memory_store())
}

/// Returns an in-memory session store.
pub fn shared_memory_store() -> SharedSessionStore {
    SharedSessionStore {
        inner: Arc::new(InMemorySessionStore::new()),
    }
}

impl SharedSessionStore {
    /// Looks up the active session bound to the provided tenant + user combination.
    pub async fn find_by_user(
        &self,
        ctx: &TenantCtx,
        user: &UserId,
    ) -> Result<Option<(SessionKey, SessionData)>> {
        self.inner.find_by_user(ctx, user).map_err(Into::into)
    }

    /// Creates a new session and returns its key.
    pub async fn create_session(&self, ctx: &TenantCtx, data: SessionData) -> Result<SessionKey> {
        self.inner.create_session(ctx, data).map_err(Into::into)
    }

    /// Updates an existing session with the supplied snapshot.
    pub async fn update_session(&self, key: &SessionKey, data: SessionData) -> Result<()> {
        self.inner.update_session(key, data).map_err(Into::into)
    }
}
