use crate::provider::ProviderKey;
use dashmap::DashMap;
use std::sync::Arc;

pub trait Provider: Send + Sync {}

pub struct ProviderRegistry<P: Provider> {
    inner: DashMap<ProviderKey, Arc<P>>,
}

impl<P: Provider> ProviderRegistry<P> {
    pub fn new() -> Self {
        Self {
            inner: DashMap::new(),
        }
    }

    pub fn get(&self, key: &ProviderKey) -> Option<Arc<P>> {
        self.inner.get(key).map(|entry| Arc::clone(entry.value()))
    }

    pub fn put(&self, key: ProviderKey, provider: Arc<P>) {
        self.inner.insert(key, provider);
    }
}
