use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result};
use greentic_config::ConfigResolver;
use greentic_config_types::GreenticConfig;
use greentic_pack::reader::SigningPolicy;
use greentic_types::{EnvId, PackId};
use gsm_core::{
    InMemoryProviderInstallStore, ProviderExtensionsRegistry, ProviderInstallState,
    ProviderInstallStore, ProviderInstallStoreSnapshot, load_install_store_from_path,
    load_provider_extensions_from_pack_files,
};
use serde_json::{Value, json};
use time::OffsetDateTime;
use tracing::{info, warn};

#[derive(Debug, Clone)]
pub struct WorkerConfig {
    pub env: EnvId,
    pub packs_root: PathBuf,
    pub install_store_path: Option<PathBuf>,
    pub sync_interval: Duration,
    pub provision_bin: String,
    pub dry_run: bool,
}

impl WorkerConfig {
    pub fn load() -> Result<Self> {
        let resolved = ConfigResolver::new().load()?;
        Ok(Self::from_config(&resolved.config))
    }

    pub fn from_config(config: &GreenticConfig) -> Self {
        let packs_root = config.paths.greentic_root.join("packs");
        let install_store_path = default_install_store_path(config);
        Self {
            env: config.environment.env_id.clone(),
            packs_root,
            install_store_path,
            sync_interval: Duration::from_secs(15 * 60),
            provision_bin: "greentic-provision".to_string(),
            dry_run: false,
        }
    }
}

pub async fn run_worker(config: WorkerConfig) -> Result<()> {
    let pack_paths = discover_pack_files(&config.packs_root)?;
    let extensions = load_provider_extensions_from_pack_files(&config.packs_root, &pack_paths)?;
    let pack_index = build_pack_index(&pack_paths)?;
    let store = load_store(&config)?;
    let mut failures = HashMap::new();
    let mut interval = tokio::time::interval(config.sync_interval);

    info!(
        env = %config.env.as_str(),
        packs = %pack_paths.len(),
        "subscriptions worker started"
    );

    loop {
        interval.tick().await;
        let updated =
            run_sync_cycle(&store, &extensions, &pack_index, &config, &mut failures).await?;
        if updated > 0 {
            info!(updated, "subscriptions state updated");
        }
        persist_store(&config, &store)?;
    }
}

fn load_store(config: &WorkerConfig) -> Result<InMemoryProviderInstallStore> {
    if let Some(path) = config.install_store_path.as_ref() {
        match load_install_store_from_path(path) {
            Ok(store) => Ok(store),
            Err(err) => {
                warn!(error = %err, path = %path.display(), "failed to load install records");
                Ok(InMemoryProviderInstallStore::default())
            }
        }
    } else {
        Ok(InMemoryProviderInstallStore::default())
    }
}

fn persist_store(config: &WorkerConfig, store: &InMemoryProviderInstallStore) -> Result<()> {
    let Some(path) = config.install_store_path.as_ref() else {
        return Ok(());
    };
    let snapshot = ProviderInstallStoreSnapshot {
        records: Vec::new(),
        states: store.all(),
    };
    let payload = serde_json::to_string_pretty(&snapshot).context("serialize install store")?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    fs::write(path, payload).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

pub async fn run_sync_cycle(
    store: &InMemoryProviderInstallStore,
    extensions: &ProviderExtensionsRegistry,
    pack_index: &HashMap<PackId, PathBuf>,
    config: &WorkerConfig,
    failures: &mut HashMap<InstallKey, FailureState>,
) -> Result<usize> {
    let mut updated = 0;
    let installs = store.all();
    for state in installs {
        if !extensions
            .subscriptions
            .contains_key(&state.record.provider_id)
        {
            continue;
        }
        let Some(pack_path) = pack_index.get(&state.record.pack_id) else {
            warn!(
                provider_id = %state.record.provider_id,
                pack_id = %state.record.pack_id,
                "missing pack for subscriptions sync"
            );
            continue;
        };
        let key = InstallKey::from_state(&state);
        if let Some(failure) = failures.get(&key)
            && failure.next_attempt > std::time::Instant::now()
            && !config.dry_run
        {
            continue;
        }

        let sync_result = run_provision_sync(state.clone(), pack_path, config).await;
        match sync_result {
            Ok(plan) => {
                let mut state = state;
                let now = OffsetDateTime::now_utc();
                state.record.updated_at = now;
                state.record.subscriptions_state = json!({
                    "last_sync": now.format(&time::format_description::well_known::Rfc3339)
                        .unwrap_or_else(|_| now.unix_timestamp().to_string()),
                    "plan": plan,
                });
                clear_subscription_error(&mut state);
                store.insert(state);
                failures.remove(&key);
                updated += 1;
            }
            Err(err) => {
                let failure = failures.entry(key.clone()).or_default();
                failure.count += 1;
                failure.bump_backoff();
                let mut state = state;
                mark_subscription_error(&mut state, failure.count, err.to_string());
                store.insert(state);
                warn!(
                    provider_id = %key.provider_id,
                    install_id = %key.install_id,
                    error = %err,
                    "subscriptions sync failed"
                );
            }
        }
    }
    Ok(updated)
}

fn mark_subscription_error(state: &mut ProviderInstallState, count: u32, err: String) {
    if !state.record.metadata.is_object() {
        state.record.metadata = json!({});
    }
    let meta = state
        .record
        .metadata
        .as_object_mut()
        .expect("metadata object");
    meta.insert("subscriptions_failure_count".into(), json!(count));
    meta.insert("subscriptions_last_error".into(), json!(err));
    meta.insert("subscriptions_degraded".into(), json!(count >= 3));
    meta.insert(
        "subscriptions_last_failure".into(),
        json!(
            OffsetDateTime::now_utc()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_else(|_| OffsetDateTime::now_utc().unix_timestamp().to_string())
        ),
    );
}

fn clear_subscription_error(state: &mut ProviderInstallState) {
    if let Some(obj) = state.record.metadata.as_object_mut() {
        obj.remove("subscriptions_failure_count");
        obj.remove("subscriptions_last_error");
        obj.remove("subscriptions_degraded");
        obj.remove("subscriptions_last_failure");
    }
}

async fn run_provision_sync(
    state: ProviderInstallState,
    pack_path: &Path,
    config: &WorkerConfig,
) -> Result<Value> {
    let state = state.clone();
    let pack_path = pack_path.to_path_buf();
    let config = config.clone();
    tokio::task::spawn_blocking(move || {
        let mut cmd = Command::new(&config.provision_bin);
        cmd.arg("sync-subscriptions")
            .arg(&state.record.provider_id)
            .arg("--install-id")
            .arg(state.record.install_id.to_string())
            .arg("--pack")
            .arg(&pack_path)
            .arg("--env")
            .arg(state.record.tenant.env.as_str())
            .arg("--tenant")
            .arg(state.record.tenant.tenant.as_str())
            .arg("--public-base-url")
            .arg(public_base_url(&state));
        if let Some(team) = state.record.tenant.team.as_ref() {
            cmd.arg("--team").arg(team.as_str());
        }
        if config.dry_run {
            cmd.arg("--dry-run");
        }
        let output = cmd.output().context("run greentic-provision")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("greentic-provision sync-subscriptions failed: {stderr}");
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let plan: Value =
            serde_json::from_str(&stdout).context("parse sync-subscriptions output")?;
        Ok(plan)
    })
    .await
    .context("sync-subscriptions join")?
}

fn public_base_url(state: &ProviderInstallState) -> String {
    state
        .record
        .metadata
        .get("public_base_url")
        .and_then(Value::as_str)
        .unwrap_or("https://example.invalid")
        .to_string()
}

pub fn discover_pack_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    discover_pack_files_inner(root, &mut out)?;
    Ok(out)
}

fn discover_pack_files_inner(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(dir).with_context(|| format!("read packs dir {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            discover_pack_files_inner(&path, out)?;
        } else if path.extension().and_then(|s| s.to_str()) == Some("gtpack") {
            out.push(path);
        }
    }
    Ok(())
}

pub fn build_pack_index(paths: &[PathBuf]) -> Result<HashMap<PackId, PathBuf>> {
    let mut out = HashMap::new();
    for path in paths {
        let manifest = read_pack_manifest(path)?;
        out.insert(manifest.pack_id, path.clone());
    }
    Ok(out)
}

fn read_pack_manifest(path: &Path) -> Result<greentic_types::pack_manifest::PackManifest> {
    let pack = greentic_pack::reader::open_pack(path, SigningPolicy::DevOk)
        .map_err(|err| anyhow::anyhow!("open pack {}: {err:?}", path.display()))?;
    let manifest = pack
        .files
        .get("manifest.cbor")
        .context("missing manifest.cbor")?;
    greentic_types::decode_pack_manifest(manifest).context("decode pack manifest")
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct InstallKey {
    env: String,
    tenant: String,
    team: Option<String>,
    provider_id: String,
    install_id: String,
}

impl InstallKey {
    fn from_state(state: &ProviderInstallState) -> Self {
        Self {
            env: state.record.tenant.env.as_str().to_string(),
            tenant: state.record.tenant.tenant.as_str().to_string(),
            team: state
                .record
                .tenant
                .team
                .as_ref()
                .map(|t| t.as_str().to_string()),
            provider_id: state.record.provider_id.clone(),
            install_id: state.record.install_id.to_string(),
        }
    }
}

#[derive(Debug)]
pub struct FailureState {
    count: u32,
    next_attempt: std::time::Instant,
}

impl FailureState {
    fn bump_backoff(&mut self) {
        let base = Duration::from_secs(30);
        let exp = self.count.saturating_sub(1).min(10);
        let factor = 1u32 << exp;
        let delay = base
            .saturating_mul(factor)
            .min(Duration::from_secs(30 * 60));
        self.next_attempt = std::time::Instant::now() + delay;
    }
}

impl Default for FailureState {
    fn default() -> Self {
        Self {
            count: 0,
            next_attempt: std::time::Instant::now(),
        }
    }
}

fn default_install_store_path(config: &GreenticConfig) -> Option<PathBuf> {
    let path = config
        .paths
        .greentic_root
        .join(".greentic/dev/installs.json");
    path.exists().then_some(path)
}
