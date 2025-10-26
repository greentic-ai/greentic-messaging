use crate::provider::ProviderKey;
use dashmap::DashMap;
use std::sync::Arc;

pub trait Provider: Send + Sync {}

/// Thread-safe registry that stores provider instances keyed by [`ProviderKey`].
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use gsm_core::registry::{Provider, ProviderRegistry};
/// use gsm_core::{Platform, ProviderKey, EnvId, TenantId};
///
/// struct MyProvider;
/// impl Provider for MyProvider {}
///
/// let registry: ProviderRegistry<MyProvider> = ProviderRegistry::default();
/// let key = ProviderKey {
///     platform: Platform::Slack,
///     env: EnvId::from("dev"),
///     tenant: TenantId::from("acme"),
///     team: None,
/// };
/// registry.put(key.clone(), Arc::new(MyProvider));
/// assert!(registry.get(&key).is_some());
/// ```
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

impl<P: Provider> Default for ProviderRegistry<P> {
    fn default() -> Self {
        Self::new()
    }
}
