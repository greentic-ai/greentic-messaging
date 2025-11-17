//! Distributed backpressure primitives backed by JetStream key-value state.

use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration as StdDuration, Instant},
};

use anyhow::{Context, Result, anyhow};
use async_nats::jetstream::{
    Context as JsContext,
    context::KeyValueErrorKind,
    kv::{self, CreateErrorKind, UpdateErrorKind},
};
use async_trait::async_trait;
use gsm_core::telemetry::{TelemetryLabels, record_gauge};
use serde::{Deserialize, Serialize};
use time::{Duration, OffsetDateTime, serde::rfc3339};
use tokio::sync::Mutex;
use tracing::{Level, event, instrument, warn};

static RATE_LIMIT_ENV: &str = "TENANT_RATE_LIMITS";
static BACKPRESSURE_NAMESPACE_ENV: &str = "JS_KV_NAMESPACE_BACKPRESSURE";

/// How many seconds one token represents.
const TOKEN: f64 = 1.0;
const TICK_MS: i64 = 100;

fn compute_wait_secs(limit: RateLimit, tokens: f64) -> f64 {
    let missing = (TOKEN - tokens).max(0.0);
    missing / limit.rps.max(0.1)
}

fn record_backpressure_tokens(tenant: &str, tokens: f64) {
    let labels = TelemetryLabels {
        tenant: tenant.to_string(),
        platform: None,
        chat_id: None,
        msg_id: None,
        extra: Vec::new(),
    };
    record_gauge("backpressure_tokens", tokens.round() as i64, &labels);
}
#[derive(Debug, Clone, Copy)]
pub struct RateLimit {
    pub rps: f64,
    pub burst: f64,
}

impl Default for RateLimit {
    fn default() -> Self {
        Self {
            rps: 5.0,
            burst: 10.0,
        }
    }
}

#[derive(Clone)]
pub struct RateLimits {
    default: RateLimit,
    tenants: HashMap<String, RateLimit>,
}

impl RateLimits {
    pub fn from_env() -> Self {
        let default = RateLimit::default();
        let tenants = std::env::var(RATE_LIMIT_ENV)
            .ok()
            .and_then(|raw| serde_json::from_str::<HashMap<String, TenantRateLimit>>(&raw).ok())
            .map(|map| {
                map.into_iter()
                    .map(|(tenant, cfg)| {
                        (
                            tenant,
                            RateLimit {
                                rps: cfg.rps.max(0.1),
                                burst: cfg.burst.max(1.0),
                            },
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self { default, tenants }
    }

    pub fn get(&self, tenant: &str) -> RateLimit {
        self.tenants.get(tenant).copied().unwrap_or(self.default)
    }
}

#[derive(Debug, Deserialize)]
struct TenantRateLimit {
    rps: f64,
    burst: f64,
}

#[async_trait]
pub trait BackpressureLimiter: Send + Sync {
    async fn acquire(&self, tenant: &str) -> Result<Permit>;
}

#[derive(Debug)]
pub struct Permit;

impl Permit {
    fn new() -> Self {
        Self
    }
}

#[derive(Clone)]
pub struct LocalBackpressureLimiter {
    limits: Arc<RateLimits>,
    buckets: Arc<Mutex<HashMap<String, LocalBucket>>>,
}

#[derive(Debug)]
struct LocalBucket {
    tokens: f64,
    last_refill: Instant,
}

impl LocalBackpressureLimiter {
    pub fn new(limits: Arc<RateLimits>) -> Self {
        Self {
            limits,
            buckets: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn refill(tokens: f64, elapsed: StdDuration, limit: RateLimit) -> (f64, StdDuration) {
        if elapsed.is_zero() {
            return (tokens, StdDuration::from_millis(0));
        }
        let ticks = (elapsed.as_millis() as i64) / TICK_MS;
        if ticks <= 0 {
            return (tokens, StdDuration::from_millis(0));
        }
        let refill = (ticks as f64) * (limit.rps * (TICK_MS as f64 / 1000.0));
        let tokens = (tokens + refill).min(limit.burst);
        let consumed = StdDuration::from_millis((ticks * TICK_MS) as u64);
        (tokens, consumed)
    }
}

#[async_trait]
impl BackpressureLimiter for LocalBackpressureLimiter {
    async fn acquire(&self, tenant: &str) -> Result<Permit> {
        let tenant_key = tenant.to_string();
        loop {
            let limit = self.limits.get(tenant);
            let mut guard = self.buckets.lock().await;
            let bucket = guard.entry(tenant_key.clone()).or_insert(LocalBucket {
                tokens: limit.burst,
                last_refill: Instant::now(),
            });
            let now = Instant::now();
            let elapsed = now.saturating_duration_since(bucket.last_refill);
            let (filled, consumed) = Self::refill(bucket.tokens, elapsed, limit);
            if consumed > StdDuration::from_millis(0) {
                bucket.last_refill += consumed;
                bucket.tokens = filled;
            }
            if bucket.tokens >= TOKEN {
                bucket.tokens -= TOKEN;
                record_backpressure_tokens(&tenant_key, bucket.tokens);
                drop(guard);
                return Ok(Permit::new());
            }
            let wait_secs = compute_wait_secs(limit, bucket.tokens);
            if wait_secs > 1.0 {
                event!(
                    Level::INFO,
                    tenant = %tenant_key,
                    wait_secs,
                    "backpressure.waiting_for_tokens"
                );
            }
            drop(guard);
            tokio::time::sleep(StdDuration::from_secs_f64(wait_secs.max(0.1))).await;
        }
    }
}

struct RemoteBucketState {
    tokens: f64,
    last_refill: OffsetDateTime,
}

#[derive(Debug, Serialize, Deserialize)]
struct RemoteBucketPersisted {
    tokens: f64,
    #[serde(with = "rfc3339")]
    last_refill_ts: OffsetDateTime,
}

impl RemoteBucketPersisted {
    fn new(tokens: f64, now: OffsetDateTime) -> Self {
        Self {
            tokens,
            last_refill_ts: now,
        }
    }
}

pub struct JetStreamBackpressureLimiter {
    limits: Arc<RateLimits>,
    bucket: kv::Store,
    namespace: String,
}

impl JetStreamBackpressureLimiter {
    pub async fn new(js: &JsContext, namespace: &str, limits: Arc<RateLimits>) -> Result<Self> {
        let bucket = match js.get_key_value(namespace).await {
            Ok(store) => store,
            Err(err) if err.kind() == KeyValueErrorKind::GetBucket => js
                .create_key_value(kv::Config {
                    bucket: namespace.to_string(),
                    description: "backpressure rate limiter".into(),
                    history: 1,
                    max_age: StdDuration::from_secs(0),
                    ..Default::default()
                })
                .await
                .with_context(|| format!("create JetStream KV bucket {namespace}"))?,
            Err(err) => return Err(anyhow!(err).context("initializing backpressure bucket")),
        };
        Ok(Self {
            limits,
            bucket,
            namespace: namespace.to_string(),
        })
    }

    fn parse_state(&self, entry: Option<kv::Entry>, limit: RateLimit) -> RemoteBucketState {
        let now = OffsetDateTime::now_utc();
        match entry {
            Some(e) => serde_json::from_slice::<RemoteBucketPersisted>(e.value.as_ref())
                .map(|persisted| RemoteBucketState {
                    tokens: persisted.tokens.min(limit.burst),
                    last_refill: persisted.last_refill_ts,
                })
                .unwrap_or(RemoteBucketState {
                    tokens: limit.burst,
                    last_refill: now,
                }),
            None => RemoteBucketState {
                tokens: limit.burst,
                last_refill: now,
            },
        }
    }

    fn refill_tokens(
        mut state: RemoteBucketState,
        limit: RateLimit,
        now: OffsetDateTime,
    ) -> RemoteBucketState {
        if now <= state.last_refill {
            return state;
        }
        let elapsed_ms = (now - state.last_refill).whole_milliseconds();
        let ticks = (elapsed_ms / i128::from(TICK_MS)) as i64;
        if ticks <= 0 {
            return state;
        }
        let refill = (ticks as f64) * (limit.rps * (TICK_MS as f64 / 1000.0));
        state.tokens = (state.tokens + refill).min(limit.burst);
        state.last_refill += Duration::milliseconds(ticks * TICK_MS);
        state
    }

    async fn wait_for_tokens(wait_secs: f64) {
        tokio::time::sleep(StdDuration::from_secs_f64(wait_secs.max(0.1))).await;
    }
}

#[async_trait]
impl BackpressureLimiter for JetStreamBackpressureLimiter {
    #[instrument(name = "backpressure.remote.acquire", skip(self), fields(namespace = %self.namespace, tenant))]
    async fn acquire(&self, tenant: &str) -> Result<Permit> {
        let tenant_key = tenant.to_string();
        let limit = self.limits.get(tenant);
        let key = format!("rate/{tenant}");
        let mut retries = 0usize;

        loop {
            let entry = self
                .bucket
                .entry(key.as_str())
                .await
                .with_context(|| format!("load rate state for {tenant}"))?;
            let now = OffsetDateTime::now_utc();
            let mut state = self.parse_state(entry.clone(), limit);
            state = Self::refill_tokens(state, limit, now);
            if state.tokens < TOKEN {
                let wait_secs = compute_wait_secs(limit, state.tokens);
                if wait_secs > 1.0 {
                    event!(
                        Level::INFO,
                        tenant = %tenant_key,
                        wait_secs,
                        namespace = %self.namespace,
                        "backpressure.waiting_for_tokens"
                    );
                }
                Self::wait_for_tokens(wait_secs).await;
                continue;
            }
            state.tokens -= TOKEN;
            record_backpressure_tokens(&tenant_key, state.tokens);
            let persisted = RemoteBucketPersisted::new(state.tokens, state.last_refill);
            let payload = serde_json::to_vec(&persisted)?;
            match &entry {
                Some(e) => match self
                    .bucket
                    .update(key.as_str(), payload.clone().into(), e.revision)
                    .await
                {
                    Ok(_) => return Ok(Permit::new()),
                    Err(err) if err.kind() == UpdateErrorKind::WrongLastRevision => {
                        retries += 1;
                        if retries > 3 {
                            event!(
                                Level::WARN,
                                tenant = %tenant_key,
                                retries,
                                "egress.acquire_permit.cas_retry"
                            );
                        }
                        continue;
                    }
                    Err(err) => {
                        return Err(
                            anyhow!(err).context(format!("update remote rate state {tenant}"))
                        );
                    }
                },
                None => match self
                    .bucket
                    .create(key.as_str(), payload.clone().into())
                    .await
                {
                    Ok(_) => return Ok(Permit::new()),
                    Err(err) if err.kind() == CreateErrorKind::AlreadyExists => {
                        retries += 1;
                        continue;
                    }
                    Err(err) => {
                        return Err(
                            anyhow!(err).context(format!("create remote rate state {tenant}"))
                        );
                    }
                },
            }
        }
    }
}

pub struct HybridLimiter {
    remote: Option<JetStreamBackpressureLimiter>,
    local: LocalBackpressureLimiter,
    remote_failed: AtomicBool,
}

impl HybridLimiter {
    pub async fn new(js: Option<&JsContext>) -> Result<Arc<Self>> {
        let limits = Arc::new(RateLimits::from_env());
        let namespace = std::env::var(BACKPRESSURE_NAMESPACE_ENV)
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| "rate-limits".to_string());

        let remote = match js {
            Some(ctx) => {
                match JetStreamBackpressureLimiter::new(ctx, &namespace, limits.clone()).await {
                    Ok(limiter) => Some(limiter),
                    Err(err) => {
                        warn!(error = %err, "remote backpressure store unavailable, falling back to local limiter");
                        None
                    }
                }
            }
            None => None,
        };

        let local = LocalBackpressureLimiter::new(limits);
        Ok(Arc::new(Self {
            remote,
            local,
            remote_failed: AtomicBool::new(false),
        }))
    }
}

#[async_trait]
impl BackpressureLimiter for HybridLimiter {
    async fn acquire(&self, tenant: &str) -> Result<Permit> {
        if let Some(remote) = &self.remote {
            match remote.acquire(tenant).await {
                Ok(permit) => {
                    self.remote_failed.store(false, Ordering::Release);
                    return Ok(permit);
                }
                Err(err) => {
                    if !self.remote_failed.swap(true, Ordering::AcqRel) {
                        warn!(error = %err, "remote limiter failed, switching to local fallback");
                    }
                }
            }
        }
        self.local.acquire(tenant).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[tokio::test]
    async fn local_refills() {
        let limits = Arc::new(RateLimits {
            default: RateLimit {
                rps: 10.0,
                burst: 2.0,
            },
            tenants: HashMap::new(),
        });
        let limiter = LocalBackpressureLimiter::new(limits);
        let _ = limiter.acquire("t").await.unwrap();
        let _ = limiter.acquire("t").await.unwrap();
    }

    #[test]
    fn parse_rate_limits_env() {
        let _guard = env_lock().lock().unwrap();
        unsafe {
            std::env::set_var(RATE_LIMIT_ENV, r#"{ "t1": {"rps": 10, "burst": 20} }"#);
        }
        let limits = RateLimits::from_env();
        let cfg = limits.get("t1");
        assert_eq!(cfg.rps, 10.0);
        assert_eq!(cfg.burst, 20.0);
        let default = limits.get("unknown");
        assert_eq!(default.rps, 5.0);
        unsafe {
            std::env::remove_var(RATE_LIMIT_ENV);
        }
    }

    #[test]
    fn compute_wait_secs_reflects_missing_tokens() {
        let limit = RateLimit {
            rps: 2.0,
            burst: 1.0,
        };
        let wait = compute_wait_secs(limit, 0.0);
        assert!((wait - 0.5).abs() < 1e-6);

        let instant = compute_wait_secs(limit, 2.0);
        assert_eq!(instant, 0.0);
    }

    #[test]
    fn refill_respects_burst_and_elapsed_time() {
        let limit = RateLimit {
            rps: 1.0,
            burst: 2.0,
        };
        // No time elapsed, nothing changes.
        let (tokens, consumed) =
            LocalBackpressureLimiter::refill(0.5, StdDuration::from_millis(0), limit);
        assert_eq!(tokens, 0.5);
        assert_eq!(consumed, StdDuration::from_millis(0));

        // Two seconds elapsed should top up but not exceed burst.
        let (tokens, consumed) =
            LocalBackpressureLimiter::refill(0.5, StdDuration::from_secs(2), limit);
        assert_eq!(tokens, 2.0);
        assert_eq!(consumed, StdDuration::from_secs(2));
    }

    #[test]
    fn rate_limits_enforce_minimums() {
        let _guard = env_lock().lock().unwrap();
        unsafe {
            std::env::set_var(
                RATE_LIMIT_ENV,
                r#"{ "tenant": {"rps": 0.0, "burst": 0.0} }"#,
            );
        }
        let limits = RateLimits::from_env();
        let cfg = limits.get("tenant");
        assert_eq!(cfg.rps, 0.1);
        assert_eq!(cfg.burst, 1.0);
        unsafe {
            std::env::remove_var(RATE_LIMIT_ENV);
        }
    }
}
