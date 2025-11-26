pub mod adapter_registry;
pub mod config;
mod main_logic;

pub use main_logic::{process_message_internal, run};
pub use messaging_bus::{BusClient, BusError, InMemoryBusClient, NatsBusClient};
