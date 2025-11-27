pub mod config;
pub mod http;
mod main_logic;

use crate::config::GatewayConfig;
use anyhow::Result;
use async_nats::Client as NatsClient;
pub use gsm_bus::{BusClient, BusError, InMemoryBusClient, NatsBusClient};
use gsm_core::{
    AdapterRegistry, DefaultAdapterPacksConfig, WorkerClient, adapter_pack_paths_from_env,
    adapter_registry::load_adapters_from_pack_files, default_adapter_pack_paths,
};
pub use main_logic::run;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::info;

/// Builds the adapter registry from default and extra pack paths.
pub fn load_adapter_registry() -> AdapterRegistry {
    let packs_root =
        PathBuf::from(std::env::var("MESSAGING_PACKS_ROOT").unwrap_or_else(|_| "packs".into()));
    let default_packs_cfg = DefaultAdapterPacksConfig::from_env();
    let extra_paths = adapter_pack_paths_from_env();
    let mut pack_paths = default_adapter_pack_paths(packs_root.as_path(), &default_packs_cfg);
    pack_paths.extend(extra_paths.clone());
    match load_adapters_from_pack_files(&pack_paths) {
        Ok(registry) => {
            let names: Vec<_> = registry.all().into_iter().map(|a| a.name).collect();
            if names.is_empty() {
                tracing::info!(
                    install_all = default_packs_cfg.install_all,
                    selected = ?default_packs_cfg.selected,
                    extra_pack_paths = ?extra_paths,
                    "no messaging adapter packs loaded (defaults disabled or missing)"
                );
            } else {
                info!(
                    adapters = ?names,
                    install_all = default_packs_cfg.install_all,
                    selected = ?default_packs_cfg.selected,
                    extra_pack_paths = ?extra_paths,
                    "loaded messaging adapter packs"
                );
            }
            registry
        }
        Err(err) => {
            tracing::warn!(error = %err, "failed to load default messaging adapter packs");
            AdapterRegistry::default()
        }
    }
}

/// Constructs the production bus client.
pub fn make_nats_bus(client: NatsClient) -> impl BusClient {
    NatsBusClient::new(client)
}

/// Convenience to build a router for the gateway with a provided bus.
pub async fn build_router_with_bus<B: BusClient + 'static>(
    config: GatewayConfig,
    adapters: AdapterRegistry,
    bus: Arc<B>,
    worker: Option<Arc<dyn WorkerClient>>,
) -> Result<axum::Router> {
    http::build_router_with_bus(config, adapters, bus, worker).await
}
