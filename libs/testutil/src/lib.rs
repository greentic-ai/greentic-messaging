use anyhow::{Context, Result, anyhow};
use jsonschema::{Validator, validator_for};
use once_cell::sync::Lazy;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::env;
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

        let env = env::var("GREENTIC_ENV").ok();
        let tenant = env::var("TENANT").ok();
        let team = env::var("TEAM").ok();

        if let Some(credentials) = load_seed_credentials(platform)? {
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

/// Load credentials from a greentic-secrets seed file when specified.
/// Supported formats:
/// - SeedDoc with `entries: [{ uri, value, ... }]`
/// - Flat map `{ "uri": "...", "value": ... }` (single entry)
///
/// The seed file path is provided via `MESSAGING_SEED_FILE` (YAML or JSON).
fn load_seed_credentials(platform: &str) -> Result<Option<Value>> {
    let path = match env::var("MESSAGING_SEED_FILE") {
        Ok(path) => PathBuf::from(path),
        Err(_) => return Ok(None),
    };
    let absolute = absolute_path(path)?;
    let content = fs::read_to_string(&absolute)
        .with_context(|| format!("failed to read seed file {}", absolute.display()))?;
    let value: Value = if absolute
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("yaml") || ext.eq_ignore_ascii_case("yml"))
        .unwrap_or(false)
    {
        let yaml: serde_yaml_bw::Value = serde_yaml_bw::from_str(&content)
            .with_context(|| format!("failed to parse yaml {}", absolute.display()))?;
        serde_json::to_value(yaml)
            .with_context(|| format!("failed to convert yaml {}", absolute.display()))?
    } else {
        serde_json::from_str(&content)
            .with_context(|| format!("failed to parse json {}", absolute.display()))?
    };

    if let Some(entry) = value
        .get("entries")
        .and_then(|entries| entries.as_array())
        .and_then(|entries| {
            entries.iter().find(|entry| {
                entry
                    .get("uri")
                    .and_then(|uri| uri.as_str())
                    .map(|uri| uri.ends_with(&format!("{platform}.credentials.json")))
                    .unwrap_or(false)
            })
        })
        && let Some(val) = entry.get("value")
    {
        return Ok(Some(val.clone()));
    }

    if value
        .get("uri")
        .and_then(|uri| uri.as_str())
        .map(|uri| uri.ends_with(&format!("{platform}.credentials.json")))
        .unwrap_or(false)
        && let Some(val) = value.get("value")
    {
        return Ok(Some(val.clone()));
    }

    Ok(None)
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
        "secrets://{env}/{tenant}/{team}/messaging/{platform}.credentials.json"
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
