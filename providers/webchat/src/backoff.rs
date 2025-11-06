use std::time::Duration;

use tokio::time::sleep as tokio_sleep;
use uuid::Uuid;

#[cfg(test)]
const BASE_DELAY_MS: u64 = 5;
#[cfg(not(test))]
const BASE_DELAY_MS: u64 = 500;

#[cfg(test)]
const MAX_DELAY_MS: u64 = 1_000;
#[cfg(not(test))]
const MAX_DELAY_MS: u64 = 30_000;

pub async fn sleep(attempt: u32) {
    let pow = attempt.min(16); // prevent overflow
    let base = BASE_DELAY_MS.saturating_mul(1u64 << pow);
    let capped = base.min(MAX_DELAY_MS);
    let jitter_source = Uuid::new_v4().as_u128();
    let jitter = if capped == 0 {
        0
    } else {
        (jitter_source % (capped as u128)) as u64
    };
    let delay = Duration::from_millis(capped.saturating_add(jitter).min(MAX_DELAY_MS));
    tokio_sleep(delay).await;
}
