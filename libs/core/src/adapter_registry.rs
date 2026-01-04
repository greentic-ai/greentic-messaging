use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::Platform;
use crate::path_safety::normalize_under_root;
use anyhow::{Context, Result, anyhow, bail};
use greentic_pack::messaging::{
    MessagingAdapterCapabilities, MessagingAdapterKind, MessagingSection,
};
use greentic_pack::reader::{SigningPolicy, open_pack};
use greentic_types::pack_manifest::{ExtensionInline, PackManifest as GpackManifest};
use greentic_types::provider::{PROVIDER_EXTENSION_ID, ProviderExtensionInline};

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
    let safe_path = resolve_pack_path(root, path)?;
    let ext = safe_path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase());
    match ext.as_deref() {
        Some("gtpack") => adapters_from_gtpack(&safe_path),
        _ => adapters_from_pack_yaml(&safe_path),
    }
}

fn resolve_pack_path(root: &Path, path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        let canonical_path = path
            .canonicalize()
            .with_context(|| format!("failed to canonicalize {}", path.display()))?;
        Ok(canonical_path)
    } else {
        normalize_under_root(root, path)
    }
}

fn adapters_from_pack_yaml(path: &Path) -> Result<Vec<AdapterDescriptor>> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read pack file {}", path.display()))?;
    let spec: PackSpec = serde_yaml_bw::from_str(&raw)
        .with_context(|| format!("{} is not a valid pack spec", path.display()))?;
    validate_pack_spec(&spec)?;
    extract_from_sources(
        &spec.id,
        &spec.version,
        None,
        spec.messaging.as_ref(),
        Some(path),
    )
}

fn adapters_from_gtpack(path: &Path) -> Result<Vec<AdapterDescriptor>> {
    let pack = open_pack(path, SigningPolicy::DevOk)
        .map_err(|err| anyhow!(err.message))
        .with_context(|| format!("failed to open {}", path.display()))?;
    // Greentic pack reader converts gpack manifests into PackManifest and drops extensions.
    // Re-read the raw manifest to recover provider extensions while keeping legacy messaging
    // adapters for compatibility.
    // Pull manifest.cbor out of the archive without full validation to avoid reimplementing
    // the reader; fall back to the already-parsed manifest if extraction fails.
    let raw_manifest = zip_manifest_bytes(path);
    let gpack_manifest: Option<GpackManifest> = raw_manifest
        .as_ref()
        .and_then(|bytes| greentic_types::decode_pack_manifest(bytes).ok());

    let pack_id = gpack_manifest
        .as_ref()
        .map(|m| m.pack_id.to_string())
        .unwrap_or_else(|| pack.manifest.meta.pack_id.clone());
    let pack_version = gpack_manifest
        .as_ref()
        .map(|m| m.version.to_string())
        .unwrap_or_else(|| pack.manifest.meta.version.to_string());

    extract_from_sources(
        &pack_id,
        &pack_version,
        gpack_manifest.as_ref().and_then(provider_inline),
        pack.manifest.meta.messaging.as_ref(),
        Some(path),
    )
}

fn zip_manifest_bytes(path: &Path) -> Option<Vec<u8>> {
    let file = std::fs::File::open(path).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;
    let mut buf = Vec::new();
    archive
        .by_name("manifest.cbor")
        .ok()?
        .read_to_end(&mut buf)
        .ok()?;
    Some(buf)
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

fn extract_from_sources(
    pack_id: &str,
    pack_version: &str,
    provider_inline: Option<ProviderExtensionInline>,
    messaging: Option<&MessagingSection>,
    source: Option<&Path>,
) -> Result<Vec<AdapterDescriptor>> {
    if let Some(provider) = provider_inline {
        let adapters = extract_from_provider_extension(pack_id, pack_version, provider, source)?;
        if !adapters.is_empty() {
            return Ok(adapters);
        }
    }

    let legacy = extract_from_legacy_messaging(pack_id, pack_version, messaging, source)?;
    if !legacy.is_empty() {
        tracing::warn!(
            pack_id = %pack_id,
            "legacy messaging.adapters used; switch to provider extension {}",
            PROVIDER_EXTENSION_ID
        );
        return Ok(legacy);
    }

    tracing::warn!(
        pack_id = %pack_id,
        "no adapters found; add provider extension {} or legacy messaging.adapters",
        PROVIDER_EXTENSION_ID
    );
    Ok(Vec::new())
}

fn extract_from_provider_extension(
    pack_id: &str,
    pack_version: &str,
    inline: ProviderExtensionInline,
    source: Option<&Path>,
) -> Result<Vec<AdapterDescriptor>> {
    inline.validate_basic().map_err(|err| anyhow!(err))?;
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for provider in inline.providers {
        if !seen.insert(provider.provider_type.clone()) {
            bail!(
                "duplicate provider_type in extension: {}",
                provider.provider_type
            );
        }
        out.push(AdapterDescriptor {
            pack_id: pack_id.to_string(),
            pack_version: pack_version.to_string(),
            name: provider.provider_type.clone(),
            kind: MessagingAdapterKind::IngressEgress,
            component: provider.runtime.component_ref.clone(),
            default_flow: None,
            custom_flow: None,
            capabilities: provider_capabilities(&provider.capabilities),
            source: source.map(Path::to_path_buf),
        });
    }
    Ok(out)
}

fn provider_capabilities(caps: &[String]) -> Option<MessagingAdapterCapabilities> {
    if caps.is_empty() {
        return None;
    }
    Some(MessagingAdapterCapabilities {
        direction: Vec::new(),
        features: caps
            .iter()
            .filter_map(|c| map_provider_feature(c.as_str()))
            .collect(),
    })
}

fn map_provider_feature(feature: &str) -> Option<String> {
    // Minimal mapping; treat provider capabilities as-is for now.
    Some(feature.to_string())
}

fn extract_from_legacy_messaging(
    pack_id: &str,
    pack_version: &str,
    messaging: Option<&MessagingSection>,
    source: Option<&Path>,
) -> Result<Vec<AdapterDescriptor>> {
    let mut out = Vec::new();
    let messaging = match messaging {
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
            pack_id: pack_id.to_string(),
            pack_version: pack_version.to_string(),
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

fn provider_inline(manifest: &GpackManifest) -> Option<ProviderExtensionInline> {
    manifest.provider_extension_inline().cloned().or_else(|| {
        manifest
            .extensions
            .as_ref()
            .and_then(|exts| exts.get(PROVIDER_EXTENSION_ID))
            .and_then(|ext| ext.inline.as_ref())
            .and_then(|inline| match inline {
                ExtensionInline::Provider(p) => Some(p.clone()),
                _ => None,
            })
    })
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
    use greentic_pack::messaging::MessagingAdapter;
    use greentic_types::provider::{ProviderDecl, ProviderExtensionInline, ProviderRuntimeRef};
    use std::io::Write;
    use tempfile::TempDir;

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

    fn write_provider_gtpack(path: &Path, provider_type: &str, component_ref: &str) {
        use greentic_types::PackId;
        use greentic_types::pack_manifest::{
            ExtensionInline, ExtensionRef, PackKind, PackManifest, PackSignatures,
        };

        let mut extensions = std::collections::BTreeMap::new();
        extensions.insert(
            greentic_types::provider::PROVIDER_EXTENSION_ID.to_string(),
            ExtensionRef {
                kind: greentic_types::provider::PROVIDER_EXTENSION_ID.to_string(),
                version: "1.0.0".to_string(),
                digest: None,
                location: None,
                inline: Some(ExtensionInline::Provider(ProviderExtensionInline {
                    providers: vec![ProviderDecl {
                        provider_type: provider_type.to_string(),
                        capabilities: vec![],
                        ops: vec![],
                        config_schema_ref: "schemas/config.json".into(),
                        state_schema_ref: None,
                        runtime: ProviderRuntimeRef {
                            component_ref: component_ref.to_string(),
                            export: "run".into(),
                            world: "greentic:provider/schema-core@1.0.0".into(),
                        },
                        docs_ref: None,
                    }],
                    additional_fields: Default::default(),
                })),
            },
        );

        let manifest = PackManifest {
            schema_version: "pack-v1".into(),
            pack_id: PackId::new("adapter-registry.providers").unwrap(),
            version: semver::Version::new(0, 0, 1),
            kind: PackKind::Provider,
            publisher: "test".into(),
            components: Vec::new(),
            flows: Vec::new(),
            dependencies: Vec::new(),
            capabilities: Vec::new(),
            secret_requirements: Vec::new(),
            signatures: PackSignatures::default(),
            bootstrap: None,
            extensions: Some(extensions),
        };
        let manifest_bytes = greentic_types::encode_pack_manifest(&manifest).unwrap();
        // Sanity: ensure extension survives round-trip decode.
        assert!(
            greentic_types::decode_pack_manifest(&manifest_bytes)
                .unwrap()
                .provider_extension_inline()
                .is_some()
        );

        let file = std::fs::File::create(path).expect("pack file");
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file("manifest.cbor", opts)
            .expect("start manifest");
        zip.write_all(&manifest_bytes).expect("write manifest");
        zip.finish().expect("finish zip");
    }

    #[test]
    fn loads_provider_extension_gtpack() {
        let temp = TempDir::new().expect("temp dir");
        let gtpack_path = temp.path().join("provider.gtpack");
        write_provider_gtpack(&gtpack_path, "messaging.slack.bot", "slack-adapter@1.0.0");

        let registry =
            load_adapters_from_pack_files(temp.path(), std::slice::from_ref(&gtpack_path))
                .expect("load adapters");
        let adapter = registry
            .get("messaging.slack.bot")
            .expect("adapter registered from provider extension");
        assert_eq!(adapter.component, "slack-adapter@1.0.0");
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
    fn loads_gtpack_archive() {
        let temp = TempDir::new().expect("temp dir");
        let gtpack_path = temp.path().join("demo.gtpack");

        let flow_yaml = r#"id: demo-flow
type: messaging
in: start
nodes: {}
"#;
        let flow_bundle = greentic_flow::flow_bundle::FlowBundle {
            id: "demo-flow".to_string(),
            kind: "messaging".to_string(),
            entry: "start".to_string(),
            yaml: flow_yaml.to_string(),
            json: serde_json::json!({
                "id": "demo-flow",
                "type": "messaging",
                "in": "start",
                "nodes": {}
            }),
            hash_blake3: greentic_flow::flow_bundle::blake3_hex(flow_yaml),
            nodes: Vec::new(),
        };

        let wasm_path = temp.path().join("demo-component.wasm");
        std::fs::write(&wasm_path, b"00").expect("write wasm stub");

        let meta = greentic_pack::builder::PackMeta {
            pack_version: greentic_pack::builder::PACK_VERSION,
            pack_id: "gtpack-demo".to_string(),
            version: semver::Version::new(0, 0, 1),
            name: "gtpack demo".to_string(),
            kind: None,
            description: None,
            authors: Vec::new(),
            license: None,
            homepage: None,
            support: None,
            vendor: None,
            imports: Vec::new(),
            entry_flows: vec![flow_bundle.id.clone()],
            created_at_utc: "1970-01-01T00:00:00Z".to_string(),
            events: None,
            repo: None,
            messaging: Some(MessagingSection {
                adapters: Some(vec![MessagingAdapter {
                    name: "gtpack-adapter".to_string(),
                    kind: MessagingAdapterKind::IngressEgress,
                    component: "demo-component@0.0.1".to_string(),
                    default_flow: Some("flows/messaging/local/default.ygtc".to_string()),
                    custom_flow: None,
                    capabilities: None,
                }]),
            }),
            interfaces: Vec::new(),
            annotations: serde_json::Map::new(),
            distribution: None,
            components: Vec::new(),
        };

        greentic_pack::builder::PackBuilder::new(meta)
            .with_flow(flow_bundle)
            .with_component(greentic_pack::builder::ComponentArtifact {
                name: "demo-component".to_string(),
                version: semver::Version::new(0, 0, 1),
                wasm_path: wasm_path.clone(),
                schema_json: None,
                manifest_json: None,
                capabilities: None,
                world: None,
                hash_blake3: None,
            })
            .with_signing(greentic_pack::builder::Signing::Dev)
            .build(&gtpack_path)
            .expect("build gtpack");

        let registry =
            load_adapters_from_pack_files(temp.path(), std::slice::from_ref(&gtpack_path))
                .expect("load adapters");
        let adapter = registry.get("gtpack-adapter").expect("adapter registered");
        assert_eq!(adapter.pack_id, "gtpack-demo");
        assert_eq!(adapter.pack_version, "0.0.1");
        assert_eq!(adapter.component, "demo-component@0.0.1");
        assert_eq!(
            adapter.source.as_ref(),
            Some(&gtpack_path.canonicalize().unwrap())
        );
    }

    #[test]
    fn allows_absolute_pack_path_outside_root() {
        let root = TempDir::new().expect("root");
        let other = TempDir::new().expect("other");
        let pack_path = other.path().join("pack.yaml");
        std::fs::write(
            &pack_path,
            r#"
id: absolute-pack
version: 0.0.1
messaging:
  adapters:
    - name: abs-ingress
      kind: ingress
      component: abs@0.0.1
            "#,
        )
        .unwrap();

        let registry =
            adapters_from_pack_file(root.path(), &pack_path).expect("load absolute pack");
        let adapter = registry.first().expect("adapter present");
        assert_eq!(adapter.name, "abs-ingress");
        assert_eq!(
            adapter.source.as_ref().map(|p| p.canonicalize().unwrap()),
            Some(pack_path.canonicalize().unwrap())
        );
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

    fn provider_inline_single(component: &str, provider_type: &str) -> ProviderExtensionInline {
        ProviderExtensionInline {
            providers: vec![ProviderDecl {
                provider_type: provider_type.to_string(),
                capabilities: vec!["capability.test".into()],
                ops: vec![],
                config_schema_ref: "schemas/config.json".into(),
                state_schema_ref: None,
                runtime: ProviderRuntimeRef {
                    component_ref: component.to_string(),
                    export: "run".into(),
                    world: "greentic:provider/schema-core@1.0.0".into(),
                },
                docs_ref: None,
            }],
            additional_fields: Default::default(),
        }
    }

    fn legacy_messaging_single(name: &str) -> MessagingSection {
        MessagingSection {
            adapters: Some(vec![MessagingAdapter {
                name: name.to_string(),
                kind: MessagingAdapterKind::IngressEgress,
                component: format!("{name}-component@1.0.0"),
                default_flow: None,
                custom_flow: None,
                capabilities: None,
            }]),
        }
    }

    #[test]
    fn extracts_from_provider_extension() {
        let providers = provider_inline_single("comp@1.0.0", "messaging.demo");
        let adapters =
            extract_from_sources("pack.id", "1.0.0", Some(providers), None, None).expect("extract");
        assert_eq!(adapters.len(), 1);
        let adapter = &adapters[0];
        assert_eq!(adapter.name, "messaging.demo");
        assert_eq!(adapter.component, "comp@1.0.0");
        assert!(adapter.capabilities.is_some());
    }

    #[test]
    fn provider_extension_wins_over_legacy() {
        let providers = provider_inline_single("comp@1.0.0", "messaging.demo");
        let adapters = extract_from_sources(
            "pack.id",
            "1.0.0",
            Some(providers),
            Some(&legacy_messaging_single("legacy-name")),
            None,
        )
        .expect("extract");
        assert_eq!(adapters.len(), 1);
        assert_eq!(adapters[0].name, "messaging.demo");
    }

    #[test]
    fn falls_back_to_legacy_when_no_provider_extension() {
        let adapters = extract_from_sources(
            "pack.id",
            "1.0.0",
            None,
            Some(&legacy_messaging_single("legacy-name")),
            None,
        )
        .expect("extract");
        assert_eq!(adapters.len(), 1);
        assert_eq!(adapters[0].name, "legacy-name");
        assert_eq!(adapters[0].component, "legacy-name-component@1.0.0");
    }

    #[test]
    fn empty_when_no_sources() {
        let adapters = extract_from_sources("pack.id", "1.0.0", None, None, None).unwrap();
        assert!(adapters.is_empty());
    }
}
