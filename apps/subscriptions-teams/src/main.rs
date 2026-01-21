use anyhow::Result;
use gsm_subscriptions_teams::{WorkerConfig, run_worker};

#[tokio::main]
async fn main() -> Result<()> {
    let _ = tracing_subscriber::fmt().with_env_filter("info").try_init();
    let config = WorkerConfig::load()?;
    run_worker(config).await
}
