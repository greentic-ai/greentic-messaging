use std::sync::Arc;

use anyhow::Result;
use async_nats::{Client, jetstream};
use gsm_idempotency::{
    IdempotencyConfig, IdempotencyGuard, InMemoryIdemStore, NatsKvIdemStore, SharedIdemStore,
};
use tracing::warn;

pub async fn init_guard(nats: &Client) -> Result<IdempotencyGuard> {
    let cfg = IdempotencyConfig::from_env();
    let js = jetstream::new(nats.clone());
    let store: SharedIdemStore = match NatsKvIdemStore::new(&js, &cfg.namespace).await {
        Ok(store) => Arc::new(store),
        Err(err) => {
            warn!(error = %err, "idempotency store unavailable, using in-memory fallback");
            Arc::new(InMemoryIdemStore::new())
        }
    };
    Ok(IdempotencyGuard::new(store, cfg.ttl_hours))
}
