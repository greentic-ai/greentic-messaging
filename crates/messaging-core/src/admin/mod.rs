use std::{collections::HashMap, sync::Arc};

pub mod models;
pub mod plan;
pub mod router;
pub mod secrets;
pub mod traits;

#[cfg(test)]
mod tests;

use self::traits::{GlobalProvisioner, TenantProvisioner};

pub struct AdminRegistry {
    pub globals: HashMap<&'static str, Arc<dyn GlobalProvisioner>>,
    pub tenants: HashMap<&'static str, Arc<dyn TenantProvisioner>>,
}

impl AdminRegistry {
    pub fn new(
        globals: HashMap<&'static str, Arc<dyn GlobalProvisioner>>,
        tenants: HashMap<&'static str, Arc<dyn TenantProvisioner>>,
    ) -> Self {
        Self { globals, tenants }
    }

    pub fn global(&self, provider: &str) -> Option<Arc<dyn GlobalProvisioner>> {
        self.globals.get(provider).cloned()
    }

    pub fn tenant(&self, provider: &str) -> Option<Arc<dyn TenantProvisioner>> {
        self.tenants.get(provider).cloned()
    }
}

impl Default for AdminRegistry {
    fn default() -> Self {
        build_registry()
    }
}

pub fn build_registry() -> AdminRegistry {
    AdminRegistry {
        globals: HashMap::new(),
        tenants: HashMap::new(),
    }
}
