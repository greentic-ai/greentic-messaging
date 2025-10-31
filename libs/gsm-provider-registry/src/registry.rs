use std::collections::HashMap;
use std::sync::Arc;

use crate::errors::MsgError;
use crate::manifest::ProviderManifest;
use crate::traits::{ReceiveAdapter, SendAdapter};

type SendFactory = Arc<dyn Fn() -> Result<Box<dyn SendAdapter>, MsgError> + Send + Sync>;
type ReceiveFactory = Arc<dyn Fn() -> Result<Box<dyn ReceiveAdapter>, MsgError> + Send + Sync>;

/// Builder used to assemble a provider registration entry.
#[derive(Default)]
pub struct ProviderBuilder {
    send_factory: Option<SendFactory>,
    receive_factory: Option<ReceiveFactory>,
}

impl ProviderBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_send<F>(mut self, factory: F) -> Self
    where
        F: Fn() -> Result<Box<dyn SendAdapter>, MsgError> + Send + Sync + 'static,
    {
        self.send_factory = Some(Arc::new(factory));
        self
    }

    pub fn with_receive<F>(mut self, factory: F) -> Self
    where
        F: Fn() -> Result<Box<dyn ReceiveAdapter>, MsgError> + Send + Sync + 'static,
    {
        self.receive_factory = Some(Arc::new(factory));
        self
    }
}

struct ProviderEntry {
    manifest: ProviderManifest,
    send_factory: Option<SendFactory>,
    receive_factory: Option<ReceiveFactory>,
}

/// Runtime handles returned when resolving a provider.
pub struct ProviderHandles {
    pub manifest: ProviderManifest,
    pub send: Option<Box<dyn SendAdapter>>,
    pub receive: Option<Box<dyn ReceiveAdapter>>,
}

/// In-memory registry of providers keyed by manifest name.
#[derive(Default)]
pub struct ProviderRegistry {
    entries: HashMap<String, ProviderEntry>,
}

#[derive(thiserror::Error, Debug)]
pub enum RegistryError {
    #[error("provider `{0}` already registered")]
    AlreadyRegistered(String),
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &mut self,
        manifest: ProviderManifest,
        builder: ProviderBuilder,
    ) -> Result<(), RegistryError> {
        if self.entries.contains_key(&manifest.name) {
            return Err(RegistryError::AlreadyRegistered(manifest.name));
        }
        let entry = ProviderEntry {
            manifest: manifest.clone(),
            send_factory: builder.send_factory,
            receive_factory: builder.receive_factory,
        };
        self.entries.insert(manifest.name, entry);
        Ok(())
    }

    pub fn manifest(&self, name: &str) -> Option<&ProviderManifest> {
        self.entries.get(name).map(|entry| &entry.manifest)
    }

    pub fn handles(&self, name: &str) -> Option<Result<ProviderHandles, MsgError>> {
        self.entries.get(name).map(|entry| {
            let send = match &entry.send_factory {
                Some(factory) => Some(factory()?),
                None => None,
            };
            let receive = match &entry.receive_factory {
                Some(factory) => Some(factory()?),
                None => None,
            };
            Ok(ProviderHandles {
                manifest: entry.manifest.clone(),
                send,
                receive,
            })
        })
    }

    pub fn send_adapter(&self, name: &str) -> Option<Result<Box<dyn SendAdapter>, MsgError>> {
        self.entries
            .get(name)
            .and_then(|entry| entry.send_factory.as_ref().map(|factory| factory()))
    }

    pub fn receive_adapter(&self, name: &str) -> Option<Result<Box<dyn ReceiveAdapter>, MsgError>> {
        self.entries
            .get(name)
            .and_then(|entry| entry.receive_factory.as_ref().map(|factory| factory()))
    }

    pub fn names(&self) -> impl Iterator<Item = &String> {
        self.entries.keys()
    }
}
