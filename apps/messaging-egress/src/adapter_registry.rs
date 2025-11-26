use anyhow::{Context, Result};
use gsm_core::{AdapterDescriptor, AdapterRegistry};

pub struct AdapterLookup<'a> {
    registry: &'a AdapterRegistry,
}

impl<'a> AdapterLookup<'a> {
    pub fn new(registry: &'a AdapterRegistry) -> Self {
        Self { registry }
    }

    pub fn egress(&self, name: &str) -> Result<AdapterDescriptor> {
        let adapter = self
            .registry
            .get(name)
            .with_context(|| format!("adapter `{name}` not found"))?
            .clone();
        if !adapter.allows_egress() {
            anyhow::bail!("adapter `{name}` does not support egress");
        }
        Ok(adapter)
    }

    pub fn default_for_platform(&self, platform: &str) -> Result<AdapterDescriptor> {
        let candidates: Vec<_> = self
            .registry
            .all()
            .into_iter()
            .filter(|a| a.allows_egress())
            .filter(|a| {
                gsm_core::infer_platform_from_adapter_name(&a.name)
                    .map(|p| p.as_str() == platform)
                    .unwrap_or(false)
            })
            .collect();
        candidates
            .into_iter()
            .next()
            .context(format!("no egress adapter found for platform `{platform}`"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gsm_core::{MessagingAdapterKind, Platform};

    fn adapter(name: &str, kind: MessagingAdapterKind) -> AdapterDescriptor {
        AdapterDescriptor {
            pack_id: "pack".into(),
            pack_version: "1.0.0".into(),
            name: name.into(),
            kind,
            component: "comp@1.0.0".into(),
            default_flow: None,
            custom_flow: None,
            capabilities: None,
            source: None,
        }
    }

    #[test]
    fn picks_adapter_by_name_override() {
        let mut registry = AdapterRegistry::default();
        registry
            .register(adapter("slack-main", MessagingAdapterKind::IngressEgress))
            .unwrap();
        let lookup = AdapterLookup::new(&registry);
        let resolved = lookup.egress("slack-main").unwrap();
        assert_eq!(resolved.name, "slack-main");
    }

    #[test]
    fn picks_adapter_by_platform_when_override_absent() {
        let mut registry = AdapterRegistry::default();
        registry
            .register(adapter("slack-main", MessagingAdapterKind::IngressEgress))
            .unwrap();
        let lookup = AdapterLookup::new(&registry);
        let resolved = lookup
            .default_for_platform(Platform::Slack.as_str())
            .unwrap();
        assert_eq!(resolved.name, "slack-main");
    }

    #[test]
    fn errors_when_no_match() {
        let registry = AdapterRegistry::default();
        let lookup = AdapterLookup::new(&registry);
        assert!(lookup.default_for_platform("slack").is_err());
    }
}
