use anyhow::{Context, Result, anyhow};
use jsonschema::JSONSchema;
use once_cell::sync::Lazy;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[cfg(feature = "e2e")]
mod assertions;
#[cfg(feature = "e2e")]
pub mod e2e;
#[cfg(feature = "e2e")]
pub mod secrets;
#[cfg(feature = "visual")]
pub mod visual;

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
    let var = format!("MESSAGING_{}_CREDENTIALS", key);
    if let Ok(raw) = std::env::var(&var) {
        if raw.trim().is_empty() {
            return Ok(None);
        }
        let json =
            serde_json::from_str(&raw).with_context(|| format!("failed to parse {}", var))?;
        return Ok(Some(json));
    }

    let path_var = format!("MESSAGING_{}_CREDENTIALS_PATH", key);
    if let Ok(path) = std::env::var(&path_var) {
        let content =
            fs::read_to_string(&path).with_context(|| format!("failed to read {}", path))?;
        let json =
            serde_json::from_str(&content).with_context(|| format!("failed to parse {}", path))?;
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

    let file = Path::new(&root)
        .join(env)
        .join(tenant)
        .join(team)
        .join("messaging")
        .join(format!("{platform}-{team}-credentials.json"));

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
        other => Err(anyhow!("unsupported fixture extension: {}", other)),
    }
}

fn absolute_path<P>(path: P) -> Result<PathBuf>
where
    P: AsRef<Path>,
{
    let relative = path.as_ref();
    if relative.is_absolute() {
        return Ok(relative.to_path_buf());
    }

    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut current = Some(manifest_dir.to_path_buf());
    while let Some(dir) = current {
        let candidate = dir.join(relative);
        if candidate.exists() {
            let canonical = candidate.canonicalize().unwrap_or(candidate);
            return Ok(canonical);
        }
        current = dir.parent().map(|p| p.to_path_buf());
    }

    Ok(manifest_dir.join(relative))
}

pub fn assert_matches_schema<P>(schema_path: P, value: &Value) -> Result<()>
where
    P: AsRef<Path>,
{
    let compiled = load_compiled_schema(schema_path.as_ref())?;

    if let Err(errors) = compiled.validate(value) {
        let mut messages: Vec<String> = Vec::new();
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

fn load_compiled_schema(path: &Path) -> Result<Arc<JSONSchema>> {
    static CACHE: Lazy<Mutex<HashMap<PathBuf, Arc<JSONSchema>>>> =
        Lazy::new(|| Mutex::new(HashMap::new()));

    let absolute = absolute_path(path)?;

    {
        let cache = CACHE.lock().unwrap();
        if let Some(schema) = cache.get(&absolute) {
            return Ok(schema.clone());
        }
    }

    let schema_value = load_schema(&absolute)?;
    let leaked: &'static Value = Box::leak(Box::new(schema_value));
    let compiled = JSONSchema::compile(leaked)
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
