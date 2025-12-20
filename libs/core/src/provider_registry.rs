use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::provider_capabilities::ProviderCapabilitiesV1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapsSource {
    FromProviderCall,
    FromPackManifest,
    Override,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderRecord {
    pub id: String,
    pub version: String,
    pub caps_source: CapsSource,
    pub capabilities: ProviderCapabilitiesV1,
    #[serde(default)]
    pub encoder_ref: Option<String>,
}

#[derive(Default)]
pub struct ProviderCapsRegistry {
    providers: HashMap<String, ProviderRecord>,
}

impl ProviderCapsRegistry {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            providers: HashMap::new(),
        }
    }

    pub fn register_provider(
        &mut self,
        id: impl Into<String>,
        version: impl Into<String>,
        caps_source: CapsSource,
        capabilities: ProviderCapabilitiesV1,
        encoder_ref: Option<String>,
    ) {
        let id = id.into();
        let record = ProviderRecord {
            id: id.clone(),
            version: version.into(),
            caps_source,
            capabilities,
            encoder_ref,
        };
        self.providers.insert(id, record);
    }

    pub fn get_caps(&self, provider_id: &str) -> Option<&ProviderCapabilitiesV1> {
        self.providers.get(provider_id).map(|p| &p.capabilities)
    }

    pub fn get(&self, provider_id: &str) -> Option<&ProviderRecord> {
        self.providers.get(provider_id)
    }
}
