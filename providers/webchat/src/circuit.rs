use std::time::{Duration, Instant};

use metrics::counter;
use tracing::{debug, info, warn};

#[derive(Clone, Debug)]
pub struct CircuitSettings {
    pub failure_threshold: u32,
    pub open_duration: Duration,
}

impl Default for CircuitSettings {
    fn default() -> Self {
        Self {
            failure_threshold: 5,
            open_duration: Duration::from_secs(30),
        }
    }
}

#[derive(Debug)]
enum CircuitState {
    Closed { consecutive_failures: u32 },
    HalfOpen,
    Open { reopen_at: Instant },
}

pub struct CircuitBreaker {
    state: CircuitState,
    settings: CircuitSettings,
    labels: CircuitLabels,
}

impl CircuitBreaker {
    pub fn new(settings: CircuitSettings, labels: CircuitLabels) -> Self {
        Self {
            state: CircuitState::Closed {
                consecutive_failures: 0,
            },
            settings,
            labels,
        }
    }

    pub async fn before_request(&mut self) {
        if let CircuitState::Open { reopen_at } = self.state {
            let now = Instant::now();
            if reopen_at > now {
                let sleep = reopen_at - now;
                debug!(?sleep, "circuit breaker sleeping before half-open probe");
                tokio::time::sleep(sleep).await;
            }
            self.state = CircuitState::HalfOpen;
            info!(
                env = self.labels.env,
                tenant = self.labels.tenant,
                team = self.labels.team,
                conversation_id = self.labels.conversation_id,
                "circuit breaker half-open probe"
            );
        }
    }

    pub fn on_success(&mut self) {
        match self.state {
            CircuitState::Closed {
                ref mut consecutive_failures,
            } => {
                if *consecutive_failures > 0 {
                    debug!(
                        failures = *consecutive_failures,
                        "resetting failure counter"
                    );
                }
                *consecutive_failures = 0;
            }
            CircuitState::HalfOpen | CircuitState::Open { .. } => {
                info!(
                    env = self.labels.env,
                    tenant = self.labels.tenant,
                    team = self.labels.team,
                    conversation_id = self.labels.conversation_id,
                    "circuit breaker closed"
                );
                counter!(
                    "webchat_circuit_events_total",
                    "state" => "closed",
                    "env" => self.labels.env.clone(),
                    "tenant" => self.labels.tenant.clone(),
                    "team" => self.labels.team.clone(),
                    "conversation" => self.labels.conversation_id.clone(),
                )
                .increment(1);
                self.state = CircuitState::Closed {
                    consecutive_failures: 0,
                };
            }
        }
    }

    pub fn on_failure(&mut self) {
        match self.state {
            CircuitState::Closed {
                ref mut consecutive_failures,
            } => {
                *consecutive_failures += 1;
                if *consecutive_failures >= self.settings.failure_threshold {
                    self.open();
                }
            }
            CircuitState::HalfOpen => {
                self.open();
            }
            CircuitState::Open { .. } => {}
        }
    }

    fn open(&mut self) {
        let reopen_at = Instant::now() + self.settings.open_duration;
        self.state = CircuitState::Open { reopen_at };
        warn!(
            env = self.labels.env,
            tenant = self.labels.tenant,
            team = self.labels.team,
            conversation_id = self.labels.conversation_id,
            reopen_in = ?self.settings.open_duration,
            "circuit breaker opened"
        );
        counter!(
            "webchat_circuit_events_total",
            "state" => "open",
            "env" => self.labels.env.clone(),
            "tenant" => self.labels.tenant.clone(),
            "team" => self.labels.team.clone(),
            "conversation" => self.labels.conversation_id.clone(),
        )
        .increment(1);
    }
}

#[derive(Clone, Debug)]
pub struct CircuitLabels {
    pub env: String,
    pub tenant: String,
    pub team: String,
    pub conversation_id: String,
}

impl CircuitLabels {
    pub fn new(env: String, tenant: String, team: String, conversation_id: String) -> Self {
        Self {
            env,
            tenant,
            team,
            conversation_id,
        }
    }
}
