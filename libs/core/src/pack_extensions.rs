use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use greentic_types::pack_manifest::{ExtensionInline, ExtensionRef, PackManifest};
use serde::{Deserialize, Serialize};

use crate::path_safety::normalize_under_root;

pub const INGRESS_EXTENSION_ID: &str = "messaging.provider_ingress.v1";
pub const OAUTH_EXTENSION_ID: &str = "messaging.oauth.v1";
pub const SUBSCRIPTIONS_EXTENSION_ID: &str = "messaging.subscriptions.v1";

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RuntimeRef {
    pub component_ref: String,
    pub export: String,
    pub world: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct IngressCapabilities {
    #[serde(default)]
    pub supports_webhook_validation: bool,
    #[serde(default)]
    pub content_types: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct IngressProviderDecl {
    pub runtime: RuntimeRef,
    #[serde(default)]
    pub capabilities: IngressCapabilities,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct OAuthProviderDecl {
    pub provider: String,
    #[serde(default)]
    pub scopes: Vec<String>,
    #[serde(default)]
    pub resource: Option<String>,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub redirect_path: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SubscriptionsProviderDecl {
    pub runtime: RuntimeRef,
    #[serde(default)]
    pub resources: Vec<String>,
    #[serde(default)]
    pub renewal_window_hours: Option<u32>,
}

#[derive(Clone, Debug, Default)]
pub struct ProviderExtensionsRegistry {
    pub ingress: BTreeMap<String, IngressProviderDecl>,
    pub oauth: BTreeMap<String, OAuthProviderDecl>,
    pub subscriptions: BTreeMap<String, SubscriptionsProviderDecl>,
}

impl ProviderExtensionsRegistry {
    pub fn is_empty(&self) -> bool {
        self.ingress.is_empty() && self.oauth.is_empty() && self.subscriptions.is_empty()
    }
}

#[derive(Debug, Deserialize)]
struct PackSpecExtensions {
    #[allow(dead_code)]
    id: String,
    #[allow(dead_code)]
    version: String,
    #[serde(default)]
    extensions: Option<BTreeMap<String, ExtensionRef>>,
}

#[derive(Debug, Deserialize)]
struct IngressPayload {
    #[serde(flatten)]
    providers: BTreeMap<String, IngressProviderDecl>,
}

#[derive(Debug, Deserialize)]
struct OAuthPayload {
    #[serde(flatten)]
    providers: BTreeMap<String, OAuthProviderDecl>,
}

#[derive(Debug, Deserialize)]
struct SubscriptionsPayload {
    #[serde(flatten)]
    providers: BTreeMap<String, SubscriptionsProviderDecl>,
}

pub fn load_provider_extensions_from_pack_files(
    root: &Path,
    paths: &[PathBuf],
) -> Result<ProviderExtensionsRegistry> {
    let root = root
        .canonicalize()
        .with_context(|| format!("failed to canonicalize packs root {}", root.display()))?;
    let mut registry = ProviderExtensionsRegistry::default();
    for path in paths {
        let extensions = extensions_from_pack_file(&root, path)
            .with_context(|| format!("failed to read pack extensions from {}", path.display()))?;
        merge_registry(&mut registry, extensions);
    }
    Ok(registry)
}

fn merge_registry(target: &mut ProviderExtensionsRegistry, incoming: ProviderExtensionsRegistry) {
    target.ingress.extend(incoming.ingress);
    target.oauth.extend(incoming.oauth);
    target.subscriptions.extend(incoming.subscriptions);
}

fn extensions_from_pack_file(root: &Path, path: &Path) -> Result<ProviderExtensionsRegistry> {
    let safe_path = if path.is_absolute() {
        path.canonicalize()
            .with_context(|| format!("failed to canonicalize {}", path.display()))?
    } else {
        normalize_under_root(root, path)?
    };
    let ext = safe_path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase());
    match ext.as_deref() {
        Some("gtpack") => extensions_from_gtpack(&safe_path),
        _ => extensions_from_pack_yaml(&safe_path),
    }
}

fn extensions_from_pack_yaml(path: &Path) -> Result<ProviderExtensionsRegistry> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read pack file {}", path.display()))?;
    let spec: PackSpecExtensions = serde_yaml_bw::from_str(&raw)
        .with_context(|| format!("{} is not a valid pack spec", path.display()))?;
    let mut registry = ProviderExtensionsRegistry::default();
    apply_extensions(&mut registry, spec.extensions.as_ref());
    Ok(registry)
}

fn extensions_from_gtpack(path: &Path) -> Result<ProviderExtensionsRegistry> {
    let manifest = decode_pack_manifest(path).with_context(|| {
        format!(
            "failed to decode manifest.cbor (extensions are required) from {}",
            path.display()
        )
    })?;
    let mut registry = ProviderExtensionsRegistry::default();
    apply_extensions(&mut registry, manifest.extensions.as_ref());
    Ok(registry)
}

fn decode_pack_manifest(path: &Path) -> Result<PackManifest> {
    let file = std::fs::File::open(path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    let mut buf = Vec::new();
    archive.by_name("manifest.cbor")?.read_to_end(&mut buf)?;
    greentic_types::decode_pack_manifest(&buf).context("invalid pack manifest")
}

fn apply_extensions(
    registry: &mut ProviderExtensionsRegistry,
    extensions: Option<&BTreeMap<String, ExtensionRef>>,
) {
    let Some(extensions) = extensions else {
        return;
    };
    if let Some(payload) = extract_ingress(extensions) {
        registry.ingress.extend(payload);
    }
    if let Some(payload) = extract_oauth(extensions) {
        registry.oauth.extend(payload);
    }
    if let Some(payload) = extract_subscriptions(extensions) {
        registry.subscriptions.extend(payload);
    }
}

fn extract_ingress(
    extensions: &BTreeMap<String, ExtensionRef>,
) -> Option<BTreeMap<String, IngressProviderDecl>> {
    let entry = extensions.get(INGRESS_EXTENSION_ID)?;
    let inline = entry.inline.as_ref()?;
    let ExtensionInline::Other(value) = inline else {
        return None;
    };
    let payload: IngressPayload = serde_json::from_value(value.clone()).ok()?;
    Some(payload.providers)
}

fn extract_oauth(
    extensions: &BTreeMap<String, ExtensionRef>,
) -> Option<BTreeMap<String, OAuthProviderDecl>> {
    let entry = extensions.get(OAUTH_EXTENSION_ID)?;
    let inline = entry.inline.as_ref()?;
    let ExtensionInline::Other(value) = inline else {
        return None;
    };
    let payload: OAuthPayload = serde_json::from_value(value.clone()).ok()?;
    Some(payload.providers)
}

fn extract_subscriptions(
    extensions: &BTreeMap<String, ExtensionRef>,
) -> Option<BTreeMap<String, SubscriptionsProviderDecl>> {
    let entry = extensions.get(SUBSCRIPTIONS_EXTENSION_ID)?;
    let inline = entry.inline.as_ref()?;
    let ExtensionInline::Other(value) = inline else {
        return None;
    };
    let payload: SubscriptionsPayload = serde_json::from_value(value.clone()).ok()?;
    Some(payload.providers)
}
