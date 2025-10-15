#![cfg(feature = "chaos")]

use std::sync::Arc;

use futures::stream::{self, StreamExt};
use gsm_backpressure::{BackpressureLimiter, LocalBackpressureLimiter, RateLimits};
use gsm_core::{MessageEnvelope, OutKind, OutMessage, Platform};
use gsm_idempotency::{IdKey, IdempotencyGuard, InMemoryIdemStore, SharedIdemStore};
use rand::{rngs::StdRng, seq::SliceRandom, SeedableRng};
use tokio::sync::Mutex;
use tokio::time::{Duration, Instant};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "chaos"]
async fn chaos_dup_and_transient() {
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();

    let rate_cfg = serde_json::json!({
        "acme":  { "rps": 40.0, "burst": 8.0 },
        "bravo": { "rps": 40.0, "burst": 8.0 },
        "citra": { "rps": 40.0, "burst": 8.0 },
        "delta": { "rps": 40.0, "burst": 8.0 }
    });
    std::env::set_var("TENANT_RATE_LIMITS", rate_cfg.to_string());

    let harness = Arc::new(ChaosHarness::new(3));
    let mut events = build_event_stream();

    {
        let mut rng = StdRng::seed_from_u64(7);
        events.shuffle(&mut rng);
    }

    stream::iter(events)
        .for_each_concurrent(Some(16), |(env, profile)| {
            let harness = harness.clone();
            async move {
                harness.process(env, profile).await;
            }
        })
        .await;

    std::env::remove_var("TENANT_RATE_LIMITS");

    let stats = harness.snapshot().await;
    assert!(
        stats.total >= 6_000,
        "expected at least 6k events, got {}",
        stats.total
    );
    let hit_rate = stats.duplicates as f64 / stats.total as f64;
    assert!(
        hit_rate > 0.49,
        "dedupe hit-rate too low: {:.2}%",
        hit_rate * 100.0
    );
    assert!(
        stats.retries > 0,
        "expected transient retries to be observed"
    );
    assert!(
        stats.dlq > 0,
        "expected some messages to land in DLQ after max retries"
    );
    assert!(
        stats.egressed > 0,
        "expected successful deliveries, found none"
    );
    assert_eq!(
        stats.ingressed,
        stats.egressed + stats.dlq,
        "ingressed != egressed + dlq: {stats:?}"
    );
    let mut latencies = stats.latencies;
    latencies.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p50 = latencies[latencies.len() / 2];
    assert!(
        p50 < 1.0,
        "per-tenant permit p50 must be under 1s, observed {:.3}s",
        p50
    );
}

fn build_event_stream() -> Vec<(MessageEnvelope, FailureProfile)> {
    let tenants = ["acme", "bravo", "citra", "delta"];
    let mut events = Vec::with_capacity(6_000);
    for tenant in tenants {
        for idx in 0..1_500 {
            let dup_idx = idx / 2;
            let msg_id = format!("{tenant}-{}", dup_idx);
            let chat_id = format!("chat-{tenant}-{}", dup_idx % 64);
            let env = MessageEnvelope {
                tenant: tenant.to_string(),
                platform: Platform::Telegram,
                chat_id,
                user_id: format!("user-{tenant}"),
                thread_id: None,
                msg_id: msg_id.clone(),
                text: Some(format!("hello {}", dup_idx)),
                timestamp: format!("2024-03-{:02}T00:00:00Z", (dup_idx % 28) + 1),
                context: Default::default(),
            };
            let profile = if dup_idx % 41 == 0 {
                FailureProfile::Permanent
            } else if dup_idx % 19 == 0 {
                FailureProfile::Transient { remaining: 5 }
            } else if dup_idx % 7 == 0 {
                FailureProfile::Transient { remaining: 2 }
            } else {
                FailureProfile::Success
            };
            events.push((env, profile));
        }
    }
    events
}

#[derive(Clone)]
struct ChaosHarness {
    guard: IdempotencyGuard,
    limiter: LocalBackpressureLimiter,
    max_retries: usize,
    stats: Arc<Mutex<Stats>>,
}

impl ChaosHarness {
    fn new(max_retries: usize) -> Self {
        let store: SharedIdemStore = Arc::new(InMemoryIdemStore::new());
        let guard = IdempotencyGuard::new(store, 24);
        let limits = Arc::new(RateLimits::from_env());
        let limiter = LocalBackpressureLimiter::new(limits);
        Self {
            guard,
            limiter,
            max_retries,
            stats: Arc::new(Mutex::new(Stats::default())),
        }
    }

    async fn process(&self, env: MessageEnvelope, profile: FailureProfile) {
        {
            let mut stats = self.stats.lock().await;
            stats.total += 1;
        }

        let key = IdKey {
            tenant: env.tenant.clone(),
            platform: env.platform.as_str().to_string(),
            msg_id: env.msg_id.clone(),
        };
        let should_process = match self.guard.should_process(&key).await {
            Ok(value) => value,
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    tenant = %key.tenant,
                    msg_id = %key.msg_id,
                    "idempotency check failed in chaos harness; defaulting to process"
                );
                true
            }
        };
        if !should_process {
            let mut stats = self.stats.lock().await;
            stats.duplicates += 1;
            return;
        }

        {
            let mut stats = self.stats.lock().await;
            stats.ingressed += 1;
        }

        let out = envelope_to_out(&env);
        let start = Instant::now();
        let permit = match self.limiter.acquire(&env.tenant).await {
            Ok(permit) => permit,
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    tenant = %env.tenant,
                    "rate limiter acquisition failed"
                );
                return;
            }
        };
        let waited = start.elapsed();
        drop(permit);

        {
            let mut stats = self.stats.lock().await;
            stats.latencies.push(waited.as_secs_f64());
        }

        let (delivered, attempts) = self.send_with_retry(&out, profile).await;

        let mut stats = self.stats.lock().await;
        stats.retries += attempts.saturating_sub(1);
        if delivered {
            stats.egressed += 1;
        } else {
            stats.dlq += 1;
        }
    }

    async fn send_with_retry(
        &self,
        out: &OutMessage,
        mut profile: FailureProfile,
    ) -> (bool, usize) {
        for attempt in 0..=self.max_retries {
            match profile.next_result() {
                Ok(()) => return (true, attempt + 1),
                Err(ChaosError::Transient) if attempt < self.max_retries => {
                    let backoff = Duration::from_millis(10 * (attempt as u64 + 1));
                    tracing::debug!(
                        tenant = %out.tenant,
                        chat_id = %out.chat_id,
                        attempt = attempt + 1,
                        "transient send failure, backing off"
                    );
                    tokio::time::sleep(backoff).await;
                }
                Err(_) => return (false, attempt + 1),
            }
        }
        (false, self.max_retries + 1)
    }

    async fn snapshot(&self) -> Stats {
        self.stats.lock().await.clone()
    }
}

#[derive(Clone, Default, Debug)]
struct Stats {
    total: usize,
    duplicates: usize,
    ingressed: usize,
    egressed: usize,
    dlq: usize,
    retries: usize,
    latencies: Vec<f64>,
}

#[derive(Clone)]
enum FailureProfile {
    Success,
    Transient { remaining: usize },
    Permanent,
}

impl FailureProfile {
    fn next_result(&mut self) -> Result<(), ChaosError> {
        match self {
            FailureProfile::Success => Ok(()),
            FailureProfile::Transient { remaining } => {
                if *remaining > 0 {
                    *remaining -= 1;
                    Err(ChaosError::Transient)
                } else {
                    Ok(())
                }
            }
            FailureProfile::Permanent => Err(ChaosError::Permanent),
        }
    }
}

#[derive(Debug)]
enum ChaosError {
    Transient,
    Permanent,
}

fn envelope_to_out(env: &MessageEnvelope) -> OutMessage {
    OutMessage {
        tenant: env.tenant.clone(),
        platform: env.platform.clone(),
        chat_id: env.chat_id.clone(),
        thread_id: env.thread_id.clone(),
        kind: OutKind::Text,
        text: env.text.clone(),
        message_card: None,
        meta: Default::default(),
    }
}
