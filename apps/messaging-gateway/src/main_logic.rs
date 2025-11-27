use crate::config::GatewayConfig;
use crate::{NatsBusClient, build_router_with_bus, load_adapter_registry};
use anyhow::Result;
use axum::serve;
use gsm_core::{HttpWorkerClient, NatsWorkerClient, WorkerClient, WorkerTransport};
use tokio::net::TcpListener;
use tracing::info;

/// Starts the gateway HTTP server using the provided configuration.
pub async fn run(config: GatewayConfig) -> Result<()> {
    let adapter_registry = load_adapter_registry();
    let nats = async_nats::connect(&config.nats_url).await?;
    let bus = NatsBusClient::new(nats.clone());

    let worker: Option<std::sync::Arc<dyn WorkerClient>> =
        config
            .worker_routing
            .as_ref()
            .map(|routing| match routing.transport {
                WorkerTransport::Nats => {
                    let client = NatsWorkerClient::new(
                        nats.clone(),
                        routing.nats_subject.clone(),
                        routing.max_retries,
                    );
                    std::sync::Arc::new(client) as std::sync::Arc<dyn WorkerClient>
                }
                WorkerTransport::Http => {
                    let url = routing
                        .http_url
                        .as_ref()
                        .cloned()
                        .unwrap_or_else(|| "http://localhost:8081/worker".into());
                    let client = HttpWorkerClient::new(url, routing.max_retries);
                    std::sync::Arc::new(client) as std::sync::Arc<dyn WorkerClient>
                }
            });

    let router = build_router_with_bus(
        config.clone(),
        adapter_registry,
        std::sync::Arc::new(bus),
        worker,
    )
    .await?;
    let listener = TcpListener::bind(config.addr).await?;
    info!("messaging-gateway listening on {}", config.addr);

    serve(listener, router)
        .with_graceful_shutdown(async {
            tokio::signal::ctrl_c().await.ok();
        })
        .await?;

    Ok(())
}
