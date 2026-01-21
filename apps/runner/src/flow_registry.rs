use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use greentic_pack::messaging::{MessagingAdapter, MessagingSection};
use greentic_pack::reader::{SigningPolicy, open_pack};
use gsm_core::{ChannelMessage, infer_platform_from_adapter_name};

use crate::model::Flow;

#[derive(Debug, Clone)]
pub struct FlowDefinition {
    pub pack_id: String,
    #[allow(dead_code)]
    pub pack_version: String,
    pub flow_id: String,
    pub platform: Option<String>,
    pub route: Option<String>,
    pub flow: Flow,
}

#[derive(Debug, Default)]
pub struct FlowRegistry {
    flows: Vec<FlowDefinition>,
    by_route: HashMap<String, Vec<usize>>,
    by_platform: HashMap<String, Vec<usize>>,
    default_by_pack: HashMap<String, usize>,
}

impl FlowRegistry {
    pub fn load_from_paths(root: &Path, paths: &[PathBuf]) -> Result<Self> {
        let root = root
            .canonicalize()
            .with_context(|| format!("failed to canonicalize packs root {}", root.display()))?;
        let mut flows: Vec<FlowDefinition> = Vec::new();
        let mut pack_defaults: HashMap<String, String> = HashMap::new();

        for path in paths {
            let pack_path = resolve_pack_path(&root, path)?;
            let ext = pack_path
                .extension()
                .and_then(|s| s.to_str())
                .map(|s| s.to_ascii_lowercase());
            match ext.as_deref() {
                Some("gtpack") => {
                    let (pack_flows, default_id) = flows_from_gtpack(&pack_path)?;
                    if let Some(default_id) = default_id
                        && let Some(pack_id) = pack_flows.first().map(|f| f.pack_id.clone())
                    {
                        pack_defaults.insert(pack_id, default_id);
                    }
                    flows.extend(pack_flows);
                }
                _ => {
                    let (pack_flows, default_id) = flows_from_pack_yaml(&root, &pack_path)?;
                    if let Some(default_id) = default_id
                        && let Some(pack_id) = pack_flows.first().map(|f| f.pack_id.clone())
                    {
                        pack_defaults.insert(pack_id, default_id);
                    }
                    flows.extend(pack_flows);
                }
            }
        }

        flows.sort_by(|a, b| {
            (a.pack_id.as_str(), a.flow_id.as_str(), a.route.as_deref()).cmp(&(
                b.pack_id.as_str(),
                b.flow_id.as_str(),
                b.route.as_deref(),
            ))
        });

        let mut registry = FlowRegistry {
            flows,
            ..Default::default()
        };
        for (idx, flow) in registry.flows.iter().enumerate() {
            if let Some(route) = flow.route.as_ref() {
                registry
                    .by_route
                    .entry(route.clone())
                    .or_default()
                    .push(idx);
            }
            if let Some(platform) = flow.platform.as_ref() {
                registry
                    .by_platform
                    .entry(platform.clone())
                    .or_default()
                    .push(idx);
            }
        }

        for (pack_id, flow_id) in pack_defaults {
            if let Some(idx) = registry
                .flows
                .iter()
                .position(|flow| flow.pack_id == pack_id && flow.flow_id == flow_id)
            {
                registry.default_by_pack.insert(pack_id, idx);
            }
        }

        Ok(registry)
    }

    pub fn select_flow<'a>(&'a self, message: &ChannelMessage) -> Result<&'a FlowDefinition> {
        if let Some(idx) = message
            .route
            .as_ref()
            .and_then(|route| self.by_route.get(route))
            .and_then(|indexes| indexes.first().copied())
        {
            return self
                .flows
                .get(idx)
                .ok_or_else(|| anyhow!("flow index out of bounds"));
        }

        let platform = message.channel_id.as_str();
        let mut candidates = self.by_platform.get(platform).cloned().unwrap_or_default();

        if candidates.is_empty() {
            if let Some(idx) = self.default_by_pack.values().min().copied() {
                return self
                    .flows
                    .get(idx)
                    .ok_or_else(|| anyhow!("flow index out of bounds"));
            }
            bail!("no flows registered for platform {platform}");
        }

        if candidates.len() == 1 {
            return self
                .flows
                .get(candidates[0])
                .ok_or_else(|| anyhow!("flow index out of bounds"));
        }

        candidates.sort();
        for idx in &candidates {
            if let Some(flow) = self.flows.get(*idx)
                && self
                    .default_by_pack
                    .get(flow.pack_id.as_str())
                    .is_some_and(|default_idx| default_idx == idx)
            {
                return self
                    .flows
                    .get(*idx)
                    .ok_or_else(|| anyhow!("flow index out of bounds"));
            }
        }

        self.flows
            .get(candidates[0])
            .ok_or_else(|| anyhow!("flow index out of bounds"))
    }

    #[allow(dead_code)]
    pub fn get_flow(&self, flow_id: &str) -> Option<&FlowDefinition> {
        self.flows.iter().find(|flow| flow.flow_id == flow_id)
    }

    pub fn is_empty(&self) -> bool {
        self.flows.is_empty()
    }
}

#[derive(Debug, serde::Deserialize)]
struct PackSpec {
    id: String,
    version: String,
    #[serde(default)]
    messaging: Option<MessagingSection>,
}

fn resolve_pack_path(root: &Path, path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        let canonical = path
            .canonicalize()
            .with_context(|| format!("failed to canonicalize {}", path.display()))?;
        Ok(canonical)
    } else {
        gsm_core::path_safety::normalize_under_root(root, path)
    }
}

fn flows_from_pack_yaml(root: &Path, path: &Path) -> Result<(Vec<FlowDefinition>, Option<String>)> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read pack file {}", path.display()))?;
    let spec: PackSpec = serde_yaml_bw::from_str(&raw)
        .with_context(|| format!("{} is not a valid pack spec", path.display()))?;

    let mut flows = Vec::new();
    let mut default_flow: Option<String> = spec
        .messaging
        .as_ref()
        .and_then(|m| m.adapters.as_ref())
        .and_then(|adapters| {
            adapters
                .iter()
                .find_map(|adapter| adapter.default_flow.as_ref().map(|_| adapter))
        })
        .and_then(|adapter| adapter.default_flow.clone());
    let pack_dir = path
        .parent()
        .ok_or_else(|| anyhow!("pack path missing parent: {}", path.display()))?;
    let mut adapters = spec.messaging.and_then(|m| m.adapters).unwrap_or_default();
    adapters.sort_by(|a, b| a.name.cmp(&b.name));

    let mut flow_cache: HashMap<PathBuf, Flow> = HashMap::new();

    for adapter in adapters {
        let flow_path = adapter
            .custom_flow
            .as_ref()
            .or(adapter.default_flow.as_ref());
        let Some(flow_path) = flow_path else {
            continue;
        };
        let resolved = resolve_flow_path(root, pack_dir, Path::new(flow_path))?;
        let flow = if let Some(existing) = flow_cache.get(&resolved) {
            existing.clone()
        } else {
            let loaded = Flow::load_from_file(resolved.to_str().unwrap())?;
            flow_cache.insert(resolved.clone(), loaded.clone());
            loaded
        };
        if default_flow.as_deref() == adapter.default_flow.as_deref() {
            default_flow = Some(flow.id.clone());
        }
        flows.push(flow_definition_from_adapter(
            spec.id.clone(),
            spec.version.clone(),
            &adapter,
            flow.id.clone(),
            flow,
        ));
    }

    if default_flow.is_none() {
        default_flow = flows.first().map(|flow| flow.flow_id.clone());
    }

    Ok((flows, default_flow))
}

fn flows_from_gtpack(path: &Path) -> Result<(Vec<FlowDefinition>, Option<String>)> {
    let pack = open_pack(path, SigningPolicy::DevOk)
        .map_err(|err| anyhow!(err.message))
        .with_context(|| format!("failed to open {}", path.display()))?;

    let pack_id = pack.manifest.meta.pack_id.clone();
    let pack_version = pack.manifest.meta.version.to_string();
    let mut flow_cache: HashMap<String, Flow> = HashMap::new();
    let mut flows = Vec::new();
    let mut registered: HashSet<String> = HashSet::new();

    for entry in &pack.manifest.flows {
        let yaml = pack
            .files
            .get(&entry.file_yaml)
            .ok_or_else(|| anyhow!("missing flow file {}", entry.file_yaml))?;
        let contents = String::from_utf8(yaml.clone())
            .with_context(|| format!("flow file {} is not UTF-8", entry.file_yaml))?;
        let flow = Flow::load_from_str(&entry.file_yaml, &contents)?;
        flow_cache.insert(entry.id.clone(), flow);
    }

    if let Some(messaging) = pack.manifest.meta.messaging.as_ref()
        && let Some(adapters) = messaging.adapters.as_ref()
    {
        for adapter in adapters {
            if let Some(flow_id) = resolve_flow_id_for_adapter(adapter, &pack.manifest.flows)
                && let Some(flow) = flow_cache.get(&flow_id).cloned()
            {
                flows.push(flow_definition_from_adapter(
                    pack_id.clone(),
                    pack_version.clone(),
                    adapter,
                    flow_id.clone(),
                    flow,
                ));
                registered.insert(flow_id);
            }
        }
    }

    for (flow_id, flow) in flow_cache {
        if registered.contains(&flow_id) {
            continue;
        }
        flows.push(FlowDefinition {
            pack_id: pack_id.clone(),
            pack_version: pack_version.clone(),
            flow_id,
            platform: None,
            route: None,
            flow,
        });
    }

    let default_flow = pack.manifest.meta.entry_flows.first().cloned();

    Ok((flows, default_flow))
}

fn resolve_flow_id_for_adapter(
    adapter: &MessagingAdapter,
    flows: &[greentic_pack::builder::FlowEntry],
) -> Option<String> {
    let flow_path = adapter
        .custom_flow
        .as_ref()
        .or(adapter.default_flow.as_ref())?;
    flows
        .iter()
        .find(|entry| entry.file_yaml == *flow_path)
        .map(|entry| entry.id.clone())
}

fn resolve_flow_path(root: &Path, pack_dir: &Path, path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        bail!("absolute flow paths are not allowed: {}", path.display());
    }
    let joined = pack_dir.join(path);
    let canon = joined
        .canonicalize()
        .with_context(|| format!("failed to canonicalize {}", joined.display()))?;
    if !canon.starts_with(root) {
        bail!(
            "flow path escapes packs root ({}): {}",
            root.display(),
            canon.display()
        );
    }
    Ok(canon)
}

fn flow_definition_from_adapter(
    pack_id: String,
    pack_version: String,
    adapter: &MessagingAdapter,
    flow_id: String,
    flow: Flow,
) -> FlowDefinition {
    let platform = infer_platform_from_adapter_name(&adapter.name)
        .map(|platform| platform.as_str().to_string());
    FlowDefinition {
        pack_id,
        pack_version,
        flow_id,
        platform,
        route: Some(adapter.name.clone()),
        flow,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gsm_core::{ChannelMessage, make_tenant_ctx};
    use std::fs;

    fn temp_dir() -> PathBuf {
        let base = std::env::temp_dir();
        let dir = base.join(format!("flow-registry-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_flow(dir: &Path, name: &str, id: &str) -> PathBuf {
        let path = dir.join(name);
        let contents = format!(
            r#"id: {id}
type: messaging
in: start
nodes:
  start:
    routes: []
"#
        );
        fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn selects_flow_by_route_then_platform_then_default() {
        let dir = temp_dir();
        let flow_default = write_flow(&dir, "default.ygtc", "flow-default");
        let flow_alt = write_flow(&dir, "alt.ygtc", "flow-alt");

        let pack = format!(
            r#"id: test-pack
version: 1.0.0
messaging:
  adapters:
    - name: slack-main
      kind: ingress-egress
      component: slack-adapter@1.0.0
      default_flow: {}
    - name: slack-alt
      kind: ingress-egress
      component: slack-adapter@1.0.0
      default_flow: {}
"#,
            flow_default.file_name().unwrap().to_string_lossy(),
            flow_alt.file_name().unwrap().to_string_lossy()
        );
        let pack_path = dir.join("pack.yaml");
        fs::write(&pack_path, pack).unwrap();

        let registry = FlowRegistry::load_from_paths(&dir, &[PathBuf::from("pack.yaml")]).unwrap();

        let ctx = make_tenant_ctx("acme".into(), Some("team".into()), None);
        let mut message = ChannelMessage {
            tenant: ctx,
            channel_id: "slack".into(),
            session_id: "chat".into(),
            route: Some("slack-alt".into()),
            payload: serde_json::json!({
                "chat_id": "chat",
                "msg_id": "m1",
                "timestamp": "2025-01-01T00:00:00Z"
            }),
        };

        let selected = registry.select_flow(&message).unwrap();
        assert_eq!(selected.flow_id, "flow-alt");

        message.route = None;
        let selected = registry.select_flow(&message).unwrap();
        assert_eq!(selected.flow_id, "flow-default");
    }
}
