use anyhow::{Result, anyhow};
use async_trait::async_trait;

pub fn env_var_for(key: &str) -> String {
    key.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

#[async_trait]
pub trait SecretsManager: Send + Sync {
    async fn get(&self, key: &str) -> Result<Option<String>>;
    async fn put(&self, key: &str, value: &str) -> Result<()>;
    fn can_write(&self) -> bool {
        false
    }
}

#[derive(Debug, Default)]
pub struct EnvSecretsManager;

#[async_trait]
impl SecretsManager for EnvSecretsManager {
    async fn get(&self, key: &str) -> Result<Option<String>> {
        let env_key = env_var_for(key);
        Ok(std::env::var(env_key).ok())
    }

    async fn put(&self, _key: &str, _value: &str) -> Result<()> {
        Err(anyhow!("env secrets manager is read-only"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[tokio::test]
    async fn env_manager_reads_uppercase_keys() {
        {
            let _guard = env_lock().lock().unwrap();
            unsafe {
                std::env::set_var("TENANTS_ACME_TELEGRAM_SECRET_TOKEN", "abc");
            }
        }
        let mgr = EnvSecretsManager;
        let value = mgr.get("tenants/acme/telegram/secret_token").await.unwrap();
        assert_eq!(value, Some("abc".into()));
        {
            let _guard = env_lock().lock().unwrap();
            unsafe {
                std::env::remove_var("TENANTS_ACME_TELEGRAM_SECRET_TOKEN");
            }
        }
    }

    #[tokio::test]
    async fn env_manager_is_read_only() {
        let mgr = EnvSecretsManager;
        let err = mgr.put("k", "v").await.unwrap_err();
        assert!(err.to_string().contains("read-only"));
    }
}
