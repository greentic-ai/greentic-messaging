use crate::prelude::*;
use once_cell::sync::Lazy;
use std::sync::RwLock;

/// Returns the current environment identifier, defaulting to `dev`.
pub fn current_env() -> EnvId {
    CURRENT_ENV
        .read()
        .expect("current env lock poisoned")
        .clone()
}

/// Updates the process-wide environment identifier used for tenant contexts.
pub fn set_current_env(env: EnvId) {
    *CURRENT_ENV.write().expect("current env lock poisoned") = env;
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

static CURRENT_ENV: Lazy<RwLock<EnvId>> = Lazy::new(|| RwLock::new(EnvId("dev".to_string())));
