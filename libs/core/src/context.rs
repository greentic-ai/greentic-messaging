use crate::prelude::*;
use std::env;

/// Returns the current environment identifier, defaulting to `dev`.
pub fn current_env() -> EnvId {
    EnvId(env::var("GREENTIC_ENV").unwrap_or_else(|_| "dev".to_string()))
}

/// Constructs a tenant context from the provided identifiers.
pub fn make_tenant_ctx(tenant: String, team: Option<String>, user: Option<String>) -> TenantCtx {
    let env = current_env();
    let tenant_id = TenantId(tenant);
    let mut ctx = TenantCtx::new(env, tenant_id);
    ctx = ctx.with_team(team.map(TeamId));
    ctx = ctx.with_user(user.map(UserId));
    ctx
}
