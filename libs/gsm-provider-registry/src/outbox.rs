use std::collections::hash_map::DefaultHasher;
use std::fmt::{self, Display, Formatter};
use std::hash::{Hash, Hasher};

use dashmap::DashMap;
use gsm_core::TenantCtx;

use crate::errors::MsgError;
use crate::traits::{Message, SendAdapter, SendResult};

/// Stable idempotency key derived from tenant + message data.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct IdempotencyKey(String);

impl IdempotencyKey {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn generate(ctx: &TenantCtx, msg: &Message) -> Self {
        let mut hasher = DefaultHasher::new();
        ctx.env.as_str().hash(&mut hasher);
        ctx.tenant.as_str().hash(&mut hasher);
        if let Some(team) = &ctx.team {
            team.as_str().hash(&mut hasher);
        }
        if let Some(user) = &ctx.user {
            user.as_str().hash(&mut hasher);
        }
        msg.to.hash(&mut hasher);
        msg.chat_id.hash(&mut hasher);
        msg.thread_id.hash(&mut hasher);
        if let Some(text) = &msg.text {
            text.hash(&mut hasher);
        }
        let fingerprint = hasher.finish();
        Self(format!("{:x}", fingerprint))
    }
}

impl Display for IdempotencyKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Minimal trait for storing outbound send outcomes.
pub trait OutboxStore: Send + Sync {
    fn lookup(&self, key: &IdempotencyKey) -> Option<SendResult>;
    fn persist(&self, key: IdempotencyKey, result: SendResult);
}

/// Simple in-memory outbox suitable for tests.
#[derive(Default)]
pub struct InMemoryOutbox {
    inner: DashMap<IdempotencyKey, SendResult>,
}

impl InMemoryOutbox {
    pub fn new() -> Self {
        Self::default()
    }
}

impl OutboxStore for InMemoryOutbox {
    fn lookup(&self, key: &IdempotencyKey) -> Option<SendResult> {
        self.inner.get(key).map(|entry| entry.clone())
    }

    fn persist(&self, key: IdempotencyKey, result: SendResult) {
        self.inner.insert(key, result);
    }
}

/// Helper that ensures idempotent sends by consulting the provided outbox store.
pub async fn send_with_outbox<A: SendAdapter, O: OutboxStore>(
    adapter: &A,
    ctx: &TenantCtx,
    message: &Message,
    outbox: &O,
) -> Result<SendResult, MsgError> {
    let key = IdempotencyKey::generate(ctx, message);
    if let Some(existing) = outbox.lookup(&key) {
        return Ok(existing);
    }
    let result = adapter.send(ctx, message).await?;
    outbox.persist(key, result.clone());
    Ok(result)
}
