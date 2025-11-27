use anyhow::Result;
use gsm_telemetry::install as init_telemetry;

use gsm_gateway::{config::GatewayConfig, run};

#[tokio::main]
async fn main() -> Result<()> {
    init_telemetry("gsm-gateway")?;

    let config = GatewayConfig::from_env()?;
    run(config).await
}
