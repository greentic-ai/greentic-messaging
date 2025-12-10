use anyhow::{Context, Result, anyhow};
use jsonschema::{Validator, validator_for};
use once_cell::sync::Lazy;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

mod path_safety;

#[cfg(feature = "e2e")]
mod assertions;
#[cfg(feature = "e2e")]
pub mod e2e;
#[cfg(feature = "e2e")]
pub mod secrets;
#[cfg(feature = "visual")]
pub mod visual;

fn workspace_root() -> PathBuf {
    // workspace root is two levels up from this crate's manifest (libs/testutil)
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .to_path_buf()
}

#[derive(Debug, Clone)]
pub struct TestConfig {
    pub platform: String,
    pub env: Option<String>,
    pub tenant: Option<String>,
    pub team: Option<String>,
    pub credentials: Option<Value>,
    pub secret_uri: Option<String>,
}

impl TestConfig {
    pub fn from_env_or_secrets(platform: &str) -> Result<Option<Self>> {
        // Load .env files when present so local credentials become available in tests.
        dotenvy::dotenv().ok();

        let env = std::env::var("GREENTIC_ENV").ok();
        let tenant = std::env::var("TENANT").ok();
        let team = std::env::var("TEAM").ok();
        let upper = platform.to_ascii_uppercase();

        if let Some(credentials) = load_env_credentials(&upper)? {
            let secret_uri =
                build_secret_uri(env.as_deref(), tenant.as_deref(), team.as_deref(), platform);
            return Ok(Some(Self {
                platform: platform.to_string(),
                env,
                tenant,
                team,
                credentials: Some(credentials),
                secret_uri,
            }));
        }

        if let Some(credentials) =
            load_secret_credentials(platform, env.as_deref(), tenant.as_deref(), team.as_deref())?
        {
            let secret_uri =
                build_secret_uri(env.as_deref(), tenant.as_deref(), team.as_deref(), platform);
            return Ok(Some(Self {
                platform: platform.to_string(),
                env,
                tenant,
                team,
                credentials: Some(credentials),
                secret_uri,
            }));
        }

        Ok(None)
    }
}

fn load_env_credentials(key: &str) -> Result<Option<Value>> {
    let var = format!("MESSAGING_{key}_CREDENTIALS");
    if let Ok(raw) = std::env::var(&var) {
        if raw.trim().is_empty() {
            return Ok(None);
        }
        let json = serde_json::from_str(&raw).with_context(|| format!("failed to parse {var}"))?;
        return Ok(Some(json));
    }

    let path_var = format!("MESSAGING_{key}_CREDENTIALS_PATH");
    if let Ok(path) = std::env::var(&path_var) {
        let credentials_path = PathBuf::from(path);
        let safe_path = absolute_path(credentials_path)?;
        let content = fs::read_to_string(&safe_path)
            .with_context(|| format!("failed to read {}", safe_path.display()))?;
        let json = serde_json::from_str(&content)
            .with_context(|| format!("failed to parse {}", safe_path.display()))?;
        return Ok(Some(json));
    }

    Ok(None)
}

fn load_secret_credentials(
    platform: &str,
    env: Option<&str>,
    tenant: Option<&str>,
    team: Option<&str>,
) -> Result<Option<Value>> {
    let env = match env {
        Some(value) => value,
        None => return Ok(None),
    };
    let tenant = match tenant {
        Some(value) => value,
        None => return Ok(None),
    };
    let team = team.unwrap_or("default");

    let root =
        match std::env::var("GREENTIC_SECRETS_DIR").or_else(|_| std::env::var("SECRETS_ROOT")) {
            Ok(value) => value,
            Err(_) => return Ok(None),
        };

    let root = PathBuf::from(root)
        .canonicalize()
        .with_context(|| "failed to canonicalize secrets root")?;
    let relative = Path::new(env)
        .join(tenant)
        .join(team)
        .join("messaging")
        .join(format!("{platform}-{team}-credentials.json"));
    let file = path_safety::normalize_under_root(&root, &relative)?;

    if !file.exists() {
        return Ok(None);
    }

    let content =
        fs::read_to_string(&file).with_context(|| format!("failed to read {}", file.display()))?;
    let json = serde_json::from_str(&content)
        .with_context(|| format!("failed to parse {}", file.display()))?;
    Ok(Some(json))
}

fn build_secret_uri(
    env: Option<&str>,
    tenant: Option<&str>,
    team: Option<&str>,
    platform: &str,
) -> Option<String> {
    let env = env?;
    let tenant = tenant?;
    let team = team.unwrap_or("default");
    Some(format!(
        "secret://{env}/{tenant}/{team}/messaging/{platform}-{team}-credentials.json"
    ))
}

pub fn load_card_value(path: &str) -> Result<Value> {
    let absolute = absolute_path(path)?;
    let content = fs::read_to_string(&absolute)
        .with_context(|| format!("failed to read {}", absolute.display()))?;
    let extension = absolute
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();

    match extension.as_str() {
        "json" => serde_json::from_str(&content)
            .with_context(|| format!("failed to parse json {}", absolute.display())),
        "yaml" | "yml" => {
            let yaml: serde_yaml_bw::Value = serde_yaml_bw::from_str(&content)
                .with_context(|| format!("failed to parse yaml {}", absolute.display()))?;
            serde_json::to_value(yaml)
                .with_context(|| format!("failed to convert yaml {}", absolute.display()))
        }
        other => Err(anyhow!("unsupported fixture extension: {other}")),
    }
}

fn absolute_path<P>(path: P) -> Result<PathBuf>
where
    P: AsRef<Path>,
{
    let root = workspace_root();
    let relative = path.as_ref();
    if relative.is_absolute() {
        let canonical = relative
            .canonicalize()
            .with_context(|| format!("failed to canonicalize {}", relative.display()))?;
        if !canonical.starts_with(&root) {
            anyhow::bail!(
                "absolute path escapes workspace root ({}): {}",
                root.display(),
                canonical.display()
            );
        }
        return Ok(canonical);
    }

    path_safety::normalize_under_root(&root, relative)
}

pub fn assert_matches_schema<P>(schema_path: P, value: &Value) -> Result<()>
where
    P: AsRef<Path>,
{
    let compiled = load_compiled_schema(schema_path.as_ref())?;

    let mut errors = compiled.iter_errors(value);
    if let Some(first) = errors.next() {
        let mut messages: Vec<String> = Vec::new();
        messages.push(first.to_string());
        for err in errors {
            messages.push(err.to_string());
        }
        return Err(anyhow!("schema validation failed: {}", messages.join("; ")));
    }

    Ok(())
}

fn load_schema(path: &Path) -> Result<Value> {
    let absolute = absolute_path(path)?;
    let content = fs::read_to_string(&absolute)
        .with_context(|| format!("failed to read {}", absolute.display()))?;
    serde_json::from_str(&content)
        .with_context(|| format!("failed to parse json {}", absolute.display()))
}

fn load_compiled_schema(path: &Path) -> Result<Arc<Validator>> {
    static CACHE: Lazy<Mutex<HashMap<PathBuf, Arc<Validator>>>> =
        Lazy::new(|| Mutex::new(HashMap::new()));

    let absolute = absolute_path(path)?;

    {
        let cache = CACHE.lock().unwrap();
        if let Some(schema) = cache.get(&absolute) {
            return Ok(schema.clone());
        }
    }

    let schema_value = load_schema(&absolute)?;
    let compiled = validator_for(&schema_value)
        .map_err(|err| anyhow!("failed to compile json schema: {err}"))?;
    let compiled = Arc::new(compiled);

    let mut cache = CACHE.lock().unwrap();
    let entry = cache.entry(absolute).or_insert_with(|| compiled.clone());
    Ok(entry.clone())
}

pub fn to_json_value<T>(value: &T) -> Result<Value>
where
    T: Serialize,
{
    serde_json::to_value(value).context("failed to convert to json value")
}

#[macro_export]
macro_rules! skip_or_require {
    ($expr:expr $(,)?) => {{
        match $expr {
            Ok(Some(value)) => value,
            Ok(None) => {
                eprintln!("skipping test: required secrets not available");
                return;
            }
            Err(err) => panic!("failed to load test secrets: {err:?}"),
        }
    }};
    ($expr:expr, $($msg:tt)+) => {{
        match $expr {
            Ok(Some(value)) => value,
            Ok(None) => {
                eprintln!("skipping test: {}", format!($($msg)+));
                return;
            }
            Err(err) => panic!("failed to load test secrets: {err:?}"),
        }
    }};
}

#[macro_export]
macro_rules! load_card {
    ($path:expr $(,)?) => {{
        $crate::load_card_value($path)
            .unwrap_or_else(|err| panic!("failed to load card {}: {}", $path, err))
    }};
}

#[macro_export]
macro_rules! assert_snapshot_json {
    ($name:expr, $value:expr $(,)?) => {{
        let snapshot_value = $crate::to_json_value(&$value)
            .unwrap_or_else(|err| panic!("failed to serialise snapshot {}: {}", $name, err));
        insta::assert_json_snapshot!($name, snapshot_value);
    }};
}
