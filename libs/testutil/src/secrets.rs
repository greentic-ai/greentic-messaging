use anyhow::{Result, anyhow};
use secrets_core::DefaultResolver;
use secrets_core::embedded::SecretsError;
use std::sync::Arc;

/// Attempts to create a `DefaultResolver`, falling back to environment variables when unavailable.
pub async fn resolver() -> Result<Option<Arc<DefaultResolver>>> {
    match DefaultResolver::new().await {
        Ok(resolver) => Ok(Some(Arc::new(resolver))),
        Err(SecretsError::Builder(_)) => {
            eprintln!("secrets resolver unavailable: falling back to environment variables");
            Ok(None)
        }
        Err(err) => {
            if is_probe_failure(&err) {
                eprintln!("secrets resolver probe failed: {}", err);
                Ok(None)
            } else {
                Err(anyhow!(err))
            }
        }
    }
}

fn is_probe_failure(err: &SecretsError) -> bool {
    matches!(err, SecretsError::Core(core) if core.to_string().contains("probe"))
}
