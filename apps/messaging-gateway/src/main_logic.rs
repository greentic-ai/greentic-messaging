use crate::config::GatewayConfig;
use crate::{
    NatsBusClient, build_router_with_bus, load_adapter_registry, load_provider_extensions_registry,
};
use anyhow::{Result, anyhow};
use axum::serve;
use gsm_core::{
    HttpWorkerClient, NatsWorkerClient, WorkerClient, WorkerRoutingConfig, WorkerTransport,
};
use tokio::net::TcpListener;
use tracing::info;

/// Starts the gateway HTTP server using the provided configuration.
pub async fn run(config: GatewayConfig) -> Result<()> {
    let adapter_registry = load_adapter_registry(
        config.packs_root.as_path(),
        &config.default_packs,
        &config.extra_pack_paths,
    );
    let provider_extensions = load_provider_extensions_registry(
        config.packs_root.as_path(),
        &config.default_packs,
        &config.extra_pack_paths,
    );
    let nats = async_nats::connect(&config.nats_url).await?;
    let bus = NatsBusClient::new(nats.clone());

    let mut worker_clients: std::collections::BTreeMap<String, std::sync::Arc<dyn WorkerClient>> =
        std::collections::BTreeMap::new();

    let routes: Vec<WorkerRoutingConfig> = if config.worker_routes.is_empty() {
        config.worker_routing.clone().into_iter().collect()
    } else {
        config.worker_routes.values().cloned().collect()
    };

    for routing in routes {
        let client: std::sync::Arc<dyn WorkerClient> = match routing.transport {
            WorkerTransport::Nats => {
                let client = NatsWorkerClient::new(
                    nats.clone(),
                    routing.nats_subject.clone(),
                    routing.max_retries,
                );
                std::sync::Arc::new(client)
            }
            WorkerTransport::Http => {
                let url = routing
                    .http_url
                    .as_ref()
                    .filter(|u| !u.is_empty())
                    .cloned()
                    .ok_or_else(|| {
                        anyhow!("REPO_WORKER_HTTP_URL must be set when REPO_WORKER_TRANSPORT=http")
                    })?;
                let client = HttpWorkerClient::new(url, routing.max_retries);
                std::sync::Arc::new(client)
            }
        };
        worker_clients.insert(routing.worker_id.clone(), client);
    }

    let router = build_router_with_bus(
        config.clone(),
        adapter_registry,
        provider_extensions,
        std::sync::Arc::new(bus),
        worker_clients,
    )
    .await?;
    let listener = TcpListener::bind(config.addr).await?;
    info!("gsm-gateway listening on {}", config.addr);

    serve(listener, router)
        .with_graceful_shutdown(async {
            tokio::signal::ctrl_c().await.ok();
        })
        .await?;

    Ok(())
}
