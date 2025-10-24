use async_trait::async_trait;
pub use greentic_types::{
    EnvId, InvocationEnvelope, NodeError, NodeResult, TeamId, TenantCtx, TenantId, UserId,
};
pub use secrets_core::DefaultResolver;
use secrets_core::{embedded::SecretsError, errors::Error as CoreError};

#[derive(Clone, Debug)]
pub struct SecretPath(pub String);

impl SecretPath {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn to_uri(&self) -> String {
        let trimmed = self.0.trim_start_matches('/');
        format!("secret://{}", trimmed)
    }
}

#[async_trait]
pub trait SecretsResolver: Send + Sync {
    async fn get_json<T>(&self, path: &SecretPath, ctx: &TenantCtx) -> NodeResult<Option<T>>
    where
        T: serde::de::DeserializeOwned + Send;

    async fn put_json<T>(&self, path: &SecretPath, ctx: &TenantCtx, value: &T) -> NodeResult<()>
    where
        T: serde::Serialize + Sync + Send;
}

#[async_trait]
impl SecretsResolver for DefaultResolver {
    async fn get_json<T>(&self, path: &SecretPath, _ctx: &TenantCtx) -> NodeResult<Option<T>>
    where
        T: serde::de::DeserializeOwned + Send,
    {
        let uri = path.to_uri();
        match self.core().get_json::<T>(&uri).await {
            Ok(value) => Ok(Some(value)),
            Err(SecretsError::Core(CoreError::NotFound { .. })) => Ok(None),
            Err(err) => Err(NodeError::new(
                "secrets_read",
                format!("failed to fetch secret {}", path.as_str()),
            )
            .with_source(err)),
        }
    }

    async fn put_json<T>(&self, path: &SecretPath, _ctx: &TenantCtx, value: &T) -> NodeResult<()>
    where
        T: serde::Serialize + Sync + Send,
    {
        let uri = path.to_uri();
        self.core()
            .put_json(&uri, value)
            .await
            .map(|_| ())
            .map_err(|err| {
                NodeError::new(
                    "secrets_write",
                    format!("failed to store secret {}", path.as_str()),
                )
                .with_source(err)
            })
    }
}
