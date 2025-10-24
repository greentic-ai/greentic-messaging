use crate::prelude::*;
use std::env;

/// Returns the current environment identifier, defaulting to `dev`.
pub fn current_env() -> EnvId {
    EnvId(env::var("GREENTIC_ENV").unwrap_or_else(|_| "dev".to_string()))
}

/// Constructs a tenant context from the provided identifiers.
pub fn make_tenant_ctx(tenant: String, team: Option<String>, user: Option<String>) -> TenantCtx {
    TenantCtx {
        env: current_env(),
        tenant: TenantId(tenant),
        team: team.map(TeamId),
        user: user.map(UserId),
        trace_id: None,
        correlation_id: None,
        deadline_unix_ms: None,
        attempt: 0,
        idempotency_key: None,
    }
}
