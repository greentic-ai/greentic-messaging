use std::collections::HashSet;
use std::path::PathBuf;

use serde::Serialize;

use crate::providers::ProviderRegistry;
use gsm_core::pack_extensions::RuntimeRef;

#[derive(Clone, Debug)]
pub struct PlatformDescriptor {
    pub id: String,
    pub label: String,
    pub platform_key: String,
    pub provider_id: String,
    pub provider_type: String,
    pub pack_id: String,
    pub runtime: RuntimeRef,
    pub capabilities: Vec<String>,
    pub pack_spec: PathBuf,
    pub pack_root: PathBuf,
}

impl PlatformDescriptor {
    pub fn to_response(&self) -> PlatformDescriptorResponse {
        PlatformDescriptorResponse {
            id: self.id.clone(),
            label: self.label.clone(),
            platform_key: self.platform_key.clone(),
            provider_id: self.provider_id.clone(),
            provider_type: self.provider_type.clone(),
            pack_id: self.pack_id.clone(),
            pack_path: self.pack_spec.display().to_string(),
            pack_root: self.pack_root.display().to_string(),
            runtime_component: self.runtime.component_ref.clone(),
            runtime_export: self.runtime.export.clone(),
            runtime_world: self.runtime.world.clone(),
            capabilities: self.capabilities.clone(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct PlatformRegistry {
    descriptors: Vec<PlatformDescriptor>,
    errors: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct PlatformDescriptorResponse {
    pub id: String,
    pub label: String,
    pub platform_key: String,
    pub provider_id: String,
    pub provider_type: String,
    pub pack_id: String,
    pub pack_path: String,
    pub pack_root: String,
    pub runtime_component: String,
    pub runtime_export: String,
    pub runtime_world: String,
    pub capabilities: Vec<String>,
}

impl PlatformRegistry {
    pub fn from_provider_registry(registry: &ProviderRegistry) -> Self {
        let mut descriptors = Vec::new();
        let mut errors = Vec::new();
        let mut seen = HashSet::new();
        for provider in registry.entries() {
            let platform_key = platform_key_from_provider_type(&provider.provider_type);
            if platform_key.is_empty() {
                errors.push(format!("provider {} has empty platform key", provider.id));
                continue;
            }
            let descriptor_id = format!("{platform_key}:{}", provider.id);
            if !seen.insert(descriptor_id.clone()) {
                continue;
            }
            descriptors.push(PlatformDescriptor {
                id: descriptor_id,
                label: format!("{} / {}", platform_key, provider.provider_type),
                platform_key: platform_key.clone(),
                provider_id: provider.id.clone(),
                provider_type: provider.provider_type.clone(),
                pack_id: provider.pack_id.clone(),
                runtime: provider.runtime.clone(),
                capabilities: provider.capabilities.clone(),
                pack_spec: provider.pack_spec.clone(),
                pack_root: provider.pack_root.clone(),
            });
        }
        descriptors.sort_by(|a, b| a.id.cmp(&b.id));
        PlatformRegistry {
            descriptors,
            errors,
        }
    }

    pub fn descriptors(&self) -> &[PlatformDescriptor] {
        &self.descriptors
    }

    pub fn errors(&self) -> &[String] {
        &self.errors
    }
}

fn platform_key_from_provider_type(provider_type: &str) -> String {
    const PREFIX: &str = "messaging.";
    let stripped = if let Some(rest) = provider_type.strip_prefix(PREFIX) {
        rest
    } else {
        provider_type
    };
    stripped.split('.').next().unwrap_or(stripped).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_key_strips_messaging_prefix() {
        assert_eq!(
            platform_key_from_provider_type("messaging.webchat"),
            "webchat"
        );
    }

    #[test]
    fn platform_key_truncates_after_dot() {
        assert_eq!(
            platform_key_from_provider_type("messaging.whatsapp.cloud"),
            "whatsapp"
        );
    }

    #[test]
    fn platform_key_handles_non_messaging() {
        assert_eq!(platform_key_from_provider_type("custom.provider"), "custom");
    }
}
