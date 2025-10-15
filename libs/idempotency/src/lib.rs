//! Distributed idempotency helpers backed by NATS JetStream key-value buckets.
//!
//! This crate is consumed by ingress adapters as well as internal workers to
//! provide a shared deduplication layer enforced across processes.

use std::{
    fmt::{Display, Formatter},
    sync::Arc,
    time::Duration as StdDuration,
};

use anyhow::{Context, Result};
use async_nats::jetstream::{
    context::KeyValueErrorKind,
    kv::{self, CreateErrorKind},
    Context as JsContext,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use time::{Duration, OffsetDateTime};
use tokio::sync::RwLock;
use tracing::{instrument, warn};

/// Composite idempotency key per tenant/platform/message.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IdKey {
    pub tenant: String,
    pub platform: String,
    pub msg_id: String,
}

impl Display for IdKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}:{}", self.tenant, self.platform, self.msg_id)
    }
}

/// Contract implemented by idempotency stores.
#[async_trait]
pub trait IdemStore: Send + Sync {
    /// Attempts to register `key` with the provided TTL. Returns `Ok(true)` when the
    /// key did not previously exist (meaning the caller should continue processing),
    /// `Ok(false)` for a duplicate, or an error when the store was unavailable.
    async fn put_if_absent(&self, key: &str, ttl_s: u64) -> Result<bool>;
}

/// Shared trait object wrapper.
pub type SharedIdemStore = Arc<dyn IdemStore>;

/// Simple in-memory store used in tests or when JetStream is unavailable.
#[derive(Clone, Default)]
pub struct InMemoryIdemStore {
    inner: Arc<RwLock<std::collections::HashMap<String, OffsetDateTime>>>,
}

impl InMemoryIdemStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn purge_expired(&self, now: OffsetDateTime) {
        let mut guard = self.inner.write().await;
        guard.retain(|_, expires| *expires > now);
    }
}

#[async_trait]
impl IdemStore for InMemoryIdemStore {
    async fn put_if_absent(&self, key: &str, ttl_s: u64) -> Result<bool> {
        let ttl = Duration::seconds(ttl_s as i64);
        let now = OffsetDateTime::now_utc();
        let mut guard = self.inner.write().await;
        match guard.get(key) {
            Some(exp) if *exp > now => Ok(false),
            _ => {
                guard.insert(key.to_string(), now + ttl);
                Ok(true)
            }
        }
    }
}

/// JetStream-backed idempotency store.
pub struct NatsKvIdemStore {
    bucket: kv::Store,
}

impl NatsKvIdemStore {
    /// Ensures a JetStream bucket exists (or creates it) and returns a store handle.
    pub async fn new(js: &JsContext, namespace: &str) -> Result<Self> {
        let bucket = match js.get_key_value(namespace).await {
            Ok(store) => store,
            Err(err) if err.kind() == KeyValueErrorKind::GetBucket => js
                .create_key_value(kv::Config {
                    bucket: namespace.to_string(),
                    history: 1,
                    max_age: StdDuration::from_secs(0),
                    ..Default::default()
                })
                .await
                .with_context(|| format!("create JetStream KV bucket {namespace}"))?,
            Err(err) => anyhow::bail!("idempotency kv init failed: {err}"),
        };
        Ok(Self { bucket })
    }
}

#[async_trait]
impl IdemStore for NatsKvIdemStore {
    #[instrument(name = "idempotency.put_if_absent", skip(self), fields(key = %key))]
    async fn put_if_absent(&self, key: &str, ttl_s: u64) -> Result<bool> {
        let ttl = StdDuration::from_secs(ttl_s.max(1));
        let seen_at = OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string());
        let payload = serde_json::to_vec(&serde_json::json!({ "seen_at": seen_at }))?;

        match self.bucket.create_with_ttl(key, payload.into(), ttl).await {
            Ok(_) => Ok(true),
            Err(err) if err.kind() == CreateErrorKind::AlreadyExists => Ok(false),
            Err(err) => Err(anyhow::anyhow!(err)
                .context(format!("put idempotency key {key} with ttl {ttl_s}s"))),
        }
    }
}

/// Configuration derived at runtime.
#[derive(Clone)]
pub struct IdempotencyConfig {
    pub ttl_hours: u64,
    pub namespace: String,
}

impl Default for IdempotencyConfig {
    fn default() -> Self {
        Self {
            ttl_hours: 36,
            namespace: "idempotency".to_string(),
        }
    }
}

impl IdempotencyConfig {
    pub fn from_env() -> Self {
        let mut cfg = Self::default();
        if let Ok(ttl) = std::env::var("IDEMPOTENCY_TTL_HOURS") {
            if let Ok(parsed) = ttl.parse::<u64>() {
                cfg.ttl_hours = parsed.max(1);
            }
        }
        if let Ok(ns) = std::env::var("JS_KV_NAMESPACE_IDEMPOTENCY") {
            if !ns.trim().is_empty() {
                cfg.namespace = ns;
            }
        }
        cfg
    }
}

/// Guard used by ingress handlers to deduplicate envelopes.
#[derive(Clone)]
pub struct IdempotencyGuard {
    ttl_secs: u64,
    store: SharedIdemStore,
}

impl IdempotencyGuard {
    pub fn new(store: SharedIdemStore, ttl_hours: u64) -> Self {
        Self {
            store,
            ttl_secs: ttl_hours.saturating_mul(3600).max(60),
        }
    }

    /// Returns `Ok(true)` when the caller should proceed (first sighting).
    pub async fn should_process(&self, key: &IdKey) -> Result<bool> {
        let inserted = self
            .store
            .put_if_absent(&key.to_string(), self.ttl_secs)
            .await?;
        if !inserted {
            warn!(tenant = %key.tenant, platform = %key.platform, msg_id = %key.msg_id, "duplicate message dropped");
            metrics::counter!("idempotency_hit", 1, "tenant" => key.tenant.clone(), "platform" => key.platform.clone());
        }
        Ok(inserted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::Duration;

    #[tokio::test]
    async fn memory_store_dedupes() {
        let store = InMemoryIdemStore::new();
        assert!(store.put_if_absent("k", 10).await.unwrap());
        assert!(!store.put_if_absent("k", 10).await.unwrap());
        store.inner.write().await.insert(
            "expired".into(),
            OffsetDateTime::now_utc() - Duration::seconds(5),
        );
        assert!(store.put_if_absent("expired", 1).await.unwrap());
    }

    #[tokio::test]
    async fn guard_should_process() {
        let store: SharedIdemStore = Arc::new(InMemoryIdemStore::new());
        let guard = IdempotencyGuard::new(store, 1);
        let key = IdKey {
            tenant: "t1".into(),
            platform: "slack".into(),
            msg_id: "abc".into(),
        };
        assert!(guard.should_process(&key).await.unwrap());
        assert!(!guard.should_process(&key).await.unwrap());
    }
}
