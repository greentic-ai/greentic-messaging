use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::Platform;
use crate::path_safety::normalize_under_root;
use anyhow::{Context, Result, bail};
use greentic_pack::messaging::{
    MessagingAdapterCapabilities, MessagingAdapterKind, MessagingSection,
};

#[derive(Debug, serde::Deserialize)]
struct PackSpec {
    id: String,
    version: String,
    #[serde(default)]
    messaging: Option<MessagingSection>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct AdapterDescriptor {
    pub pack_id: String,
    pub pack_version: String,
    pub name: String,
    pub kind: MessagingAdapterKind,
    pub component: String,
    pub default_flow: Option<String>,
    pub custom_flow: Option<String>,
    pub capabilities: Option<MessagingAdapterCapabilities>,
    pub source: Option<PathBuf>,
}

impl AdapterDescriptor {
    /// Returns true if the adapter can be used for ingress.
    pub fn allows_ingress(&self) -> bool {
        matches!(
            self.kind,
            MessagingAdapterKind::Ingress | MessagingAdapterKind::IngressEgress
        )
    }

    /// Returns true if the adapter can be used for egress.
    pub fn allows_egress(&self) -> bool {
        matches!(
            self.kind,
            MessagingAdapterKind::Egress | MessagingAdapterKind::IngressEgress
        )
    }

    /// Returns a flow path to use, preferring custom_flow if set.
    pub fn flow_path(&self) -> Option<&str> {
        self.custom_flow.as_deref().or(self.default_flow.as_deref())
    }
}

#[derive(Default, Clone, Debug)]
pub struct AdapterRegistry {
    adapters: HashMap<String, AdapterDescriptor>,
}

impl AdapterRegistry {
    pub fn load_from_paths(root: &Path, paths: &[PathBuf]) -> Result<Self> {
        load_adapters_from_pack_files(root, paths)
    }

    pub fn register(&mut self, adapter: AdapterDescriptor) -> Result<()> {
        if self.adapters.contains_key(&adapter.name) {
            bail!("duplicate adapter registration for {}", adapter.name);
        }
        self.adapters.insert(adapter.name.clone(), adapter);
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&AdapterDescriptor> {
        self.adapters.get(name)
    }

    pub fn all(&self) -> Vec<AdapterDescriptor> {
        self.adapters.values().cloned().collect()
    }

    pub fn by_kind(&self, kind: MessagingAdapterKind) -> Vec<AdapterDescriptor> {
        self.adapters
            .values()
            .filter(|a| a.kind == kind)
            .cloned()
            .collect()
    }

    pub fn names(&self) -> Vec<String> {
        self.adapters.keys().cloned().collect()
    }

    pub fn is_empty(&self) -> bool {
        self.adapters.is_empty()
    }
}

pub fn load_adapters_from_pack_files(root: &Path, paths: &[PathBuf]) -> Result<AdapterRegistry> {
    let root = root
        .canonicalize()
        .with_context(|| format!("failed to canonicalize packs root {}", root.display()))?;
    let mut registry = AdapterRegistry::default();
    for path in paths {
        let adapters = adapters_from_pack_file(&root, path)
            .with_context(|| format!("failed to load pack {}", path.display()))?;
        for adapter in adapters {
            registry
                .register(adapter)
                .with_context(|| format!("failed to register adapters from {}", path.display()))?;
        }
    }
    Ok(registry)
}

pub fn adapters_from_pack_file(root: &Path, path: &Path) -> Result<Vec<AdapterDescriptor>> {
    let safe_path = if path.is_absolute() {
        let canonical_path = path
            .canonicalize()
            .with_context(|| format!("failed to canonicalize {}", path.display()))?;
        if !canonical_path.starts_with(root) {
            bail!(
                "pack path {} must be under {}",
                canonical_path.display(),
                root.display()
            );
        }
        canonical_path
    } else {
        normalize_under_root(root, path)?
    };
    let raw = fs::read_to_string(&safe_path)
        .with_context(|| format!("failed to read pack file {}", safe_path.display()))?;
    let spec: PackSpec = serde_yaml_bw::from_str(&raw)
        .with_context(|| format!("{} is not a valid pack spec", safe_path.display()))?;
    validate_pack_spec(&spec)?;
    extract_adapters(&spec, Some(&safe_path))
}

fn validate_pack_spec(spec: &PackSpec) -> Result<()> {
    if spec.id.trim().is_empty() {
        bail!("pack id must not be empty");
    }
    if spec.version.trim().is_empty() {
        bail!("pack version must not be empty");
    }
    if let Some(messaging) = &spec.messaging {
        messaging.validate()?;
    }
    Ok(())
}

fn extract_adapters(spec: &PackSpec, source: Option<&Path>) -> Result<Vec<AdapterDescriptor>> {
    let mut out = Vec::new();
    let messaging = match &spec.messaging {
        Some(section) => section,
        None => return Ok(out),
    };
    let adapters = match &messaging.adapters {
        Some(list) => list,
        None => return Ok(out),
    };
    // validate uniqueness (MessagingSection::validate already enforces, but keep defensive)
    let mut seen = std::collections::BTreeSet::new();
    for adapter in adapters {
        if !seen.insert(&adapter.name) {
            bail!("duplicate messaging adapter name: {}", adapter.name);
        }
        out.push(AdapterDescriptor {
            pack_id: spec.id.clone(),
            pack_version: spec.version.clone(),
            name: adapter.name.clone(),
            kind: adapter.kind.clone(),
            component: adapter.component.clone(),
            default_flow: adapter.default_flow.clone(),
            custom_flow: adapter.custom_flow.clone(),
            capabilities: adapter.capabilities.clone(),
            source: source.map(Path::to_path_buf),
        });
    }
    Ok(out)
}

/// Best-effort inference of `Platform` from an adapter name prefix.
pub fn infer_platform_from_adapter_name(name: &str) -> Option<Platform> {
    let lowered = name.to_ascii_lowercase();
    if lowered.starts_with("slack") {
        Some(Platform::Slack)
    } else if lowered.starts_with("teams") {
        Some(Platform::Teams)
    } else if lowered.starts_with("webex") {
        Some(Platform::Webex)
    } else if lowered.starts_with("webchat") {
        Some(Platform::WebChat)
    } else if lowered.starts_with("whatsapp") {
        Some(Platform::WhatsApp)
    } else if lowered.starts_with("telegram") {
        Some(Platform::Telegram)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_slack_pack() {
        let packs_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs");
        let base = packs_root
            .join("messaging/slack.yaml")
            .canonicalize()
            .expect("canonicalize pack path");
        let registry =
            load_adapters_from_pack_files(packs_root.as_path(), std::slice::from_ref(&base))
                .unwrap();
        let adapter = registry.get("slack-main").expect("adapter registered");
        assert_eq!(adapter.pack_id, "greentic-messaging-slack");
        assert_eq!(adapter.kind, MessagingAdapterKind::IngressEgress);
        assert_eq!(adapter.component, "slack-adapter@1.0.0");
        assert_eq!(
            adapter.default_flow.as_deref(),
            Some("flows/messaging/slack/default.ygtc")
        );
        assert_eq!(adapter.source.as_ref(), Some(&base));
    }

    #[test]
    fn by_kind_filters() {
        let packs_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs");
        let paths = vec![
            packs_root
                .join("messaging/slack.yaml")
                .canonicalize()
                .expect("canonicalize pack path"),
            packs_root
                .join("messaging/telegram.yaml")
                .canonicalize()
                .expect("canonicalize pack path"),
        ];
        let registry = load_adapters_from_pack_files(packs_root.as_path(), &paths).unwrap();
        let ingress = registry.by_kind(MessagingAdapterKind::Ingress);
        assert!(ingress.iter().any(|a| a.name == "telegram-ingress"));
        let egress = registry.by_kind(MessagingAdapterKind::Egress);
        assert!(egress.iter().any(|a| a.name == "telegram-egress"));
        let both = registry.by_kind(MessagingAdapterKind::IngressEgress);
        assert!(both.iter().any(|a| a.name == "slack-main"));
    }

    #[test]
    fn infers_platform_from_name_prefix() {
        assert_eq!(
            infer_platform_from_adapter_name("slack-main"),
            Some(Platform::Slack)
        );
        assert_eq!(
            infer_platform_from_adapter_name("telegram-ingress"),
            Some(Platform::Telegram)
        );
        assert_eq!(infer_platform_from_adapter_name("unknown"), None);
    }
}
