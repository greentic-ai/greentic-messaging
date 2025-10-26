//! Tenant configuration helpers for Telegram ingress bootstrap.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::{fs, path::Path};

#[derive(Debug, Clone, Deserialize)]
pub struct TelegramConfig {
    #[serde(default)]
    pub enabled: bool,
    pub public_webhook_base: String,
    pub secret_token_key: String,
    #[serde(default = "default_allowed_updates_opt")]
    pub allowed_updates: Option<Vec<String>>,
    #[serde(default = "default_drop_pending_opt")]
    pub drop_pending_on_first_install: Option<bool>,
}

impl TelegramConfig {
    pub fn allowed_updates(&self) -> Vec<String> {
        self.allowed_updates
            .clone()
            .unwrap_or_else(default_allowed_updates)
    }

    pub fn drop_pending_on_first_install(&self) -> bool {
        self.drop_pending_on_first_install
            .unwrap_or_else(default_drop_pending)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Tenant {
    pub id: String,
    #[serde(default)]
    pub telegram: Option<TelegramConfig>,
}

#[derive(Debug, Clone, Deserialize)]
struct TenantsFile {
    tenants: Vec<Tenant>,
}

pub fn load_tenants(config_path: Option<&str>, fallback_tenant: &str) -> Result<Vec<Tenant>> {
    if let Some(path) = config_path {
        let path = Path::new(path);
        if path.exists() {
            let raw = fs::read_to_string(path)
                .with_context(|| format!("read tenants config {}", path.display()))?;
            let file: TenantsFile = serde_yaml_bw::from_str(&raw)
                .with_context(|| format!("parse tenants config {}", path.display()))?;
            return Ok(file.tenants);
        }
    }

    let base = std::env::var("TELEGRAM_PUBLIC_WEBHOOK_BASE")
        .unwrap_or_else(|_| "http://localhost:8080/telegram/webhook".into());
    let secret_key = std::env::var("TELEGRAM_SECRET_TOKEN_KEY")
        .unwrap_or_else(|_| format!("tenants/{}/telegram/secret_token", fallback_tenant));

    Ok(vec![Tenant {
        id: fallback_tenant.to_string(),
        telegram: Some(TelegramConfig {
            enabled: true,
            public_webhook_base: base,
            secret_token_key: secret_key,
            allowed_updates: default_allowed_updates_opt(),
            drop_pending_on_first_install: Some(default_drop_pending()),
        }),
    }])
}

fn default_allowed_updates() -> Vec<String> {
    vec!["message".into(), "callback_query".into()]
}

fn default_allowed_updates_opt() -> Option<Vec<String>> {
    Some(default_allowed_updates())
}

fn default_drop_pending() -> bool {
    true
}

fn default_drop_pending_opt() -> Option<bool> {
    Some(default_drop_pending())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn telegram_defaults_are_applied() {
        let _guard = env_lock().lock().unwrap();
        std::env::remove_var("TELEGRAM_PUBLIC_WEBHOOK_BASE");
        std::env::remove_var("TELEGRAM_SECRET_TOKEN_KEY");

        let tenants = load_tenants(None, "acme").expect("load tenants");
        assert_eq!(tenants.len(), 1);
        let tenant = tenants.first().unwrap();
        assert_eq!(tenant.id, "acme");
        let tg = tenant.telegram.as_ref().expect("telegram config");
        assert!(tg.enabled);
        assert_eq!(
            tg.public_webhook_base,
            "http://localhost:8080/telegram/webhook"
        );
        assert_eq!(tg.secret_token_key, "tenants/acme/telegram/secret_token");
        assert_eq!(tg.allowed_updates(), vec!["message", "callback_query"]);
        assert!(tg.drop_pending_on_first_install());
    }

    #[test]
    fn load_from_file_parses_values() {
        let _guard = env_lock().lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tenants.yaml");
        fs::write(
            &path,
            r#"
tenants:
  - id: acme
    telegram:
      enabled: true
      public_webhook_base: "https://hook"
      secret_token_key: "tenants/acme/telegram/secret_token"
      allowed_updates: ["message"]
      drop_pending_on_first_install: false
"#,
        )
        .unwrap();

        let tenants =
            load_tenants(path.to_str(), "ignored").expect("load tenants from yaml should work");
        assert_eq!(tenants.len(), 1);
        let tenant = tenants.first().unwrap();
        let tg = tenant.telegram.as_ref().unwrap();
        assert_eq!(tg.public_webhook_base, "https://hook");
        assert_eq!(tg.allowed_updates(), vec!["message"]);
        assert!(!tg.drop_pending_on_first_install());
    }
}
