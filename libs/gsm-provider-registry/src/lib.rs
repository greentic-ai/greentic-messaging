//! Lightweight provider registry and manifest loader shared across messaging adapters.

pub mod errors;
pub mod manifest;
pub mod outbox;
pub mod registry;
pub mod traits;

#[cfg(feature = "webchat")]
pub mod providers;

pub use errors::MsgError;
pub use manifest::{ManifestError, ProviderManifest};
pub use outbox::{IdempotencyKey, InMemoryOutbox, OutboxStore};
pub use registry::{ProviderBuilder, ProviderHandles, ProviderRegistry, RegistryError};
pub use traits::{Message, ReceiveAdapter, SendAdapter, SendResult};
