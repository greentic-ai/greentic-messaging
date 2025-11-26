use crate::config::GatewayConfig;
use crate::{NatsBusClient, build_router_with_bus, load_adapter_registry};
use anyhow::Result;
use axum::serve;
use tokio::net::TcpListener;
use tracing::info;

/// Starts the gateway HTTP server using the provided configuration.
pub async fn run(config: GatewayConfig) -> Result<()> {
    let adapter_registry = load_adapter_registry();
    let nats = async_nats::connect(&config.nats_url).await?;
    let bus = NatsBusClient::new(nats);
    let router =
        build_router_with_bus(config.clone(), adapter_registry, std::sync::Arc::new(bus)).await?;
    let listener = TcpListener::bind(config.addr).await?;
    info!("messaging-gateway listening on {}", config.addr);

    serve(listener, router)
        .with_graceful_shutdown(async {
            tokio::signal::ctrl_c().await.ok();
        })
        .await?;

    Ok(())
}
