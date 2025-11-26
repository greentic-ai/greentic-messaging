use anyhow::Result;
use gsm_telemetry::install as init_telemetry;

use messaging_gateway::{config::GatewayConfig, run};

#[tokio::main]
async fn main() -> Result<()> {
    init_telemetry("messaging-gateway")?;

    let config = GatewayConfig::from_env()?;
    run(config).await
}
