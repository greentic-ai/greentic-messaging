use super::models::{DesiredGlobalApp, DesiredTenantBinding, ProvisionCaps, ProvisionReport};
use anyhow::Error as AnyError;
use async_trait::async_trait;
use serde::Serialize;
use url::Url;

pub type AdminResult<T> = Result<T, AdminError>;

#[derive(Debug, thiserror::Error, Serialize)]
#[serde(tag = "type", content = "detail")]
pub enum AdminError {
    #[error("validation error: {0}")]
    Validation(String),

    #[error("provider error: {0}")]
    Provider(String),

    #[error("secrets error: {0}")]
    Secrets(String),

    #[error("unauthorized")]
    Unauthorized,

    #[error("not found")]
    NotFound,

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("internal error")]
    Internal {
        #[serde(skip_serializing)]
        #[from]
        #[source]
        source: AnyError,
    },
}

#[async_trait]
pub trait GlobalProvisioner: Send + Sync + 'static {
    fn provider(&self) -> &'static str;
    fn capabilities(&self) -> ProvisionCaps;

    /// Idempotent global bootstrap. May create or patch provider app/config.
    async fn ensure_global(&self, desired: &DesiredGlobalApp) -> AdminResult<ProvisionReport>;

    /// Optional admin consent/authorization for global bootstrap.
    /// Return Ok(None) if not required.
    async fn start_global_consent(&self) -> AdminResult<Option<Url>> {
        Ok(None)
    }

    /// Handle global consent callback (e.g. OAuth redirect query params).
    async fn handle_global_callback(&self, _query: &[(String, String)]) -> AdminResult<()> {
        Ok(())
    }
}

#[async_trait]
pub trait TenantProvisioner: Send + Sync + 'static {
    fn provider(&self) -> &'static str;

    /// Optional tenant admin consent/authorization.
    /// Return Ok(None) if not required (guided ingestion).
    async fn start_tenant_consent(
        &self,
        _tenant_key: &str,
        _provider_tenant_id: &str,
    ) -> AdminResult<Option<Url>> {
        Ok(None)
    }

    /// Handle tenant consent callback (e.g. OAuth redirect query params).
    async fn handle_tenant_callback(
        &self,
        _tenant_key: &str,
        _query: &[(String, String)],
    ) -> AdminResult<()> {
        Ok(())
    }

    /// Idempotent binding (installs, subscriptions, tokens, etc).
    /// May call `plan_tenant()` internally but must then apply changes.
    /// First execution should populate `created`/`updated` entries, while
    /// subsequent executions with the same desired state should only report
    /// skipped work.
    async fn ensure_tenant(&self, desired: &DesiredTenantBinding) -> AdminResult<ProvisionReport>;

    /// Strictly side-effect free dry-run used for diff computation only.
    /// MUST NOT write secrets or create provider resources.
    async fn plan_tenant(&self, desired: &DesiredTenantBinding) -> AdminResult<ProvisionReport>;
}
