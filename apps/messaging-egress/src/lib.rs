pub mod adapter_registry;
pub mod config;
mod main_logic;

pub use gsm_bus::{BusClient, BusError, InMemoryBusClient, NatsBusClient};
pub use main_logic::{process_message_internal, run};
