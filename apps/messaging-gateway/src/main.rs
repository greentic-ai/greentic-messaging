mod config;
mod http;

use anyhow::Result;
use axum::serve;
use gsm_telemetry::install as init_telemetry;
use tokio::net::TcpListener;
use tracing::info;

use crate::config::GatewayConfig;
use crate::http::build_router;

#[tokio::main]
async fn main() -> Result<()> {
    init_telemetry("messaging-gateway")?;

    let config = GatewayConfig::from_env()?;
    let router = build_router(config.clone()).await?;
    let listener = TcpListener::bind(config.addr).await?;
    info!("messaging-gateway listening on {}", config.addr);

    serve(listener, router)
        .with_graceful_shutdown(async {
            tokio::signal::ctrl_c().await.ok();
        })
        .await?;

    Ok(())
}
