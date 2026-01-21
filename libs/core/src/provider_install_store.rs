use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::RwLock;

use anyhow::{Context, Result};
use greentic_types::{ProviderInstallId, ProviderInstallRecord, TenantCtx};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PROVIDER_ID_KEY: &str = "provider_id";
pub const INSTALL_ID_KEY: &str = "install_id";
pub const PROVIDER_CONFIG_REFS_KEY: &str = "provider_config_refs";
pub const PROVIDER_SECRET_REFS_KEY: &str = "provider_secret_refs";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderInstallState {
    pub record: ProviderInstallRecord,
    pub config: BTreeMap<String, Value>,
    pub secrets: BTreeMap<String, String>,
}

impl ProviderInstallState {
    pub fn new(record: ProviderInstallRecord) -> Self {
        Self {
            record,
            config: BTreeMap::new(),
            secrets: BTreeMap::new(),
        }
    }

    pub fn with_config(mut self, config: BTreeMap<String, Value>) -> Self {
        self.config = config;
        self
    }

    pub fn with_secrets(mut self, secrets: BTreeMap<String, String>) -> Self {
        self.secrets = secrets;
        self
    }

    pub fn routing(&self) -> Option<ProviderInstallRouting> {
        let routing = self.record.metadata.get("routing")?.clone();
        serde_json::from_value(routing).ok()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProviderInstallRouting {
    #[serde(default)]
    pub platform: Option<String>,
    #[serde(default)]
    pub channel_id: Option<String>,
}

impl ProviderInstallRouting {
    pub fn matches(&self, platform: &str, channel_id: &str) -> bool {
        let platform_ok = self
            .platform
            .as_deref()
            .map(|p| p.eq_ignore_ascii_case(platform))
            .unwrap_or(true);
        let channel_ok = self
            .channel_id
            .as_deref()
            .map(|c| c == channel_id)
            .unwrap_or(false);
        platform_ok && channel_ok
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderInstallError {
    #[error("missing provider install {provider_id}/{install_id}")]
    MissingInstall {
        provider_id: String,
        install_id: String,
    },
    #[error("invalid webhook signature (header {header})")]
    InvalidSignature { header: String },
    #[error("missing provider install secret {key}")]
    MissingSecret { key: String },
    #[error("missing provider install config {key}")]
    MissingConfig { key: String },
    #[error("missing provider install route")]
    MissingRoute,
}

pub trait ProviderInstallStore: Send + Sync {
    fn insert(&self, state: ProviderInstallState);
    fn get(
        &self,
        tenant: &TenantCtx,
        provider_id: &str,
        install_id: &ProviderInstallId,
    ) -> Option<ProviderInstallState>;
    fn find_by_routing(
        &self,
        tenant: &TenantCtx,
        provider_id: &str,
        platform: &str,
        channel_id: &str,
    ) -> Option<ProviderInstallState>;
    fn all(&self) -> Vec<ProviderInstallState>;
}

#[derive(Default)]
pub struct InMemoryProviderInstallStore {
    records: RwLock<HashMap<InstallKey, ProviderInstallState>>,
    routing: RwLock<HashMap<RoutingKey, InstallKey>>,
}

impl InMemoryProviderInstallStore {
    fn install_key(
        tenant: &TenantCtx,
        provider_id: &str,
        install_id: &ProviderInstallId,
    ) -> InstallKey {
        InstallKey {
            tenant: TenantKey::from_ctx(tenant),
            provider_id: provider_id.to_string(),
            install_id: install_id.to_string(),
        }
    }
}

impl ProviderInstallStore for InMemoryProviderInstallStore {
    fn insert(&self, state: ProviderInstallState) {
        let record = &state.record;
        let key = InstallKey {
            tenant: TenantKey::from_ctx(&record.tenant),
            provider_id: record.provider_id.clone(),
            install_id: record.install_id.to_string(),
        };
        if let Some(routing) = state.routing()
            && let (Some(platform), Some(channel_id)) =
                (routing.platform.clone(), routing.channel_id.clone())
        {
            let route_key = RoutingKey {
                tenant: key.tenant.clone(),
                provider_id: key.provider_id.clone(),
                platform,
                channel_id,
            };
            self.routing
                .write()
                .expect("routing lock poisoned")
                .insert(route_key, key.clone());
        }
        self.records
            .write()
            .expect("records lock poisoned")
            .insert(key, state);
    }

    fn get(
        &self,
        tenant: &TenantCtx,
        provider_id: &str,
        install_id: &ProviderInstallId,
    ) -> Option<ProviderInstallState> {
        let key = Self::install_key(tenant, provider_id, install_id);
        self.records
            .read()
            .expect("records lock poisoned")
            .get(&key)
            .cloned()
    }

    fn find_by_routing(
        &self,
        tenant: &TenantCtx,
        provider_id: &str,
        platform: &str,
        channel_id: &str,
    ) -> Option<ProviderInstallState> {
        let route_key = RoutingKey {
            tenant: TenantKey::from_ctx(tenant),
            provider_id: provider_id.to_string(),
            platform: platform.to_string(),
            channel_id: channel_id.to_string(),
        };
        let records = self.records.read().expect("records lock poisoned");
        let routing = self.routing.read().expect("routing lock poisoned");
        routing
            .get(&route_key)
            .and_then(|install_key| records.get(install_key).cloned())
    }

    fn all(&self) -> Vec<ProviderInstallState> {
        self.records
            .read()
            .expect("records lock poisoned")
            .values()
            .cloned()
            .collect()
    }
}

#[derive(Clone, Debug, Eq)]
struct TenantKey {
    env: String,
    tenant: String,
    team: Option<String>,
}

impl TenantKey {
    fn from_ctx(tenant: &TenantCtx) -> Self {
        Self {
            env: tenant.env.as_str().to_string(),
            tenant: tenant.tenant.as_str().to_string(),
            team: tenant.team.as_ref().map(|team| team.as_str().to_string()),
        }
    }
}

impl PartialEq for TenantKey {
    fn eq(&self, other: &Self) -> bool {
        self.env == other.env && self.tenant == other.tenant && self.team == other.team
    }
}

impl Hash for TenantKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.env.hash(state);
        self.tenant.hash(state);
        self.team.hash(state);
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
struct InstallKey {
    tenant: TenantKey,
    provider_id: String,
    install_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
struct RoutingKey {
    tenant: TenantKey,
    provider_id: String,
    platform: String,
    channel_id: String,
}

pub fn apply_install_refs(meta: &mut BTreeMap<String, Value>, record: &ProviderInstallRecord) {
    meta.insert(
        PROVIDER_ID_KEY.to_string(),
        Value::String(record.provider_id.clone()),
    );
    meta.insert(
        INSTALL_ID_KEY.to_string(),
        Value::String(record.install_id.to_string()),
    );
    if !record.config_refs.is_empty() {
        meta.insert(
            PROVIDER_CONFIG_REFS_KEY.to_string(),
            serde_json::to_value(&record.config_refs).unwrap_or(Value::Null),
        );
    }
    if !record.secret_refs.is_empty() {
        meta.insert(
            PROVIDER_SECRET_REFS_KEY.to_string(),
            serde_json::to_value(&record.secret_refs).unwrap_or(Value::Null),
        );
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderInstallStoreSnapshot {
    #[serde(default)]
    pub records: Vec<ProviderInstallRecord>,
    #[serde(default)]
    pub states: Vec<ProviderInstallState>,
}

pub fn load_install_store_from_path(path: &Path) -> Result<InMemoryProviderInstallStore> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("read install store {}", path.display()))?;
    let snapshot: ProviderInstallStoreSnapshot = serde_json::from_str(&raw)
        .with_context(|| format!("parse install store {}", path.display()))?;
    let store = InMemoryProviderInstallStore::default();
    let states = if snapshot.states.is_empty() {
        snapshot
            .records
            .into_iter()
            .map(ProviderInstallState::new)
            .collect()
    } else {
        snapshot.states
    };
    for state in states {
        store.insert(state);
    }
    Ok(store)
}

pub fn extract_provider_route(
    meta: &BTreeMap<String, Value>,
) -> Option<(String, ProviderInstallId)> {
    let provider_id = meta.get(PROVIDER_ID_KEY)?.as_str()?.to_string();
    let install_id = meta.get(INSTALL_ID_KEY)?.as_str()?.parse().ok()?;
    Some((provider_id, install_id))
}
