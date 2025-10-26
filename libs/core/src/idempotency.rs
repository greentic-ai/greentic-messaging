//! Idempotency helpers for webhook/event processing.
use std::{
    collections::HashSet,
    hash::{Hash, Hasher},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

/// Composite idempotency key identifying an incoming message uniquely.
///
/// ```
/// use gsm_core::IdKey;
///
/// let key = IdKey {
///     platform: "telegram".into(),
///     chat_id: "chat-1".into(),
///     msg_id: "msg-1".into(),
/// };
/// assert_eq!(key.platform, "telegram");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IdKey {
    pub platform: String,
    pub chat_id: String,
    pub msg_id: String,
}

/// Very small in-memory seen-set with TTL. Intended for single-process demos/tests.
#[derive(Clone)]
pub struct SeenSet {
    inner: Arc<Mutex<Inner>>,
    ttl: Duration,
}

struct Inner {
    set: HashSet<u64>,
    times: Vec<(u64, Instant)>,
}

impl SeenSet {
    /// Creates a new `SeenSet` that evicts entries after `ttl`.
    ///
    /// ```
    /// use gsm_core::{IdKey, SeenSet};
    /// use std::time::Duration;
    ///
    /// let set = SeenSet::new(Duration::from_secs(1));
    /// let key = IdKey {
    ///     platform: "telegram".into(),
    ///     chat_id: "chat-1".into(),
    ///     msg_id: "msg-1".into(),
    /// };
    /// assert!(!set.seen_or_insert(&key));
    /// assert!(set.seen_or_insert(&key));
    /// ```
    pub fn new(ttl: Duration) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                set: HashSet::new(),
                times: Vec::new(),
            })),
            ttl,
        }
    }

    fn key_hash(k: &IdKey) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        k.hash(&mut hasher);
        hasher.finish()
    }

    /// Returns `true` when the key has been seen recently, or stores it and returns `false`.
    pub fn seen_or_insert(&self, key: &IdKey) -> bool {
        let now = Instant::now();
        let mut g = self.inner.lock().unwrap();
        // GC old entries
        let mut expired = Vec::new();
        g.times.retain(|(h, t)| {
            if now.duration_since(*t) > self.ttl {
                expired.push(*h);
                false
            } else {
                true
            }
        });
        for h in expired {
            g.set.remove(&h);
        }
        let h = Self::key_hash(key);
        if g.set.contains(&h) {
            return true;
        }
        g.set.insert(h);
        g.times.push((h, now));
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn idempotency_basic() {
        let s = SeenSet::new(Duration::from_millis(50));
        let k = IdKey {
            platform: "telegram".into(),
            chat_id: "c1".into(),
            msg_id: "m1".into(),
        };
        assert!(!s.seen_or_insert(&k));
        assert!(s.seen_or_insert(&k));
    }

    #[test]
    fn idempotency_entries_expire() {
        let s = SeenSet::new(Duration::from_millis(10));
        let k = IdKey {
            platform: "telegram".into(),
            chat_id: "c1".into(),
            msg_id: "m1".into(),
        };
        assert!(!s.seen_or_insert(&k));
        std::thread::sleep(Duration::from_millis(20));
        assert!(!s.seen_or_insert(&k));
    }
}
