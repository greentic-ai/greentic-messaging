/// Returns a fully qualified global secret key name for a provider.
/// Example: "messaging/global/teams/client_id"
pub fn global_key(provider: &str, name: &str) -> String {
    format!("messaging/global/{provider}/{name}")
}

/// Returns a fully qualified tenant secret key.
/// `tenant_key` MUST equal the TenantCtx.tenant_id used elsewhere in Greentic.
/// Example: "messaging/tenant/{tenant_key}/teams/app_token"
pub fn tenant_key(tenant_key: &str, provider: &str, name: &str) -> String {
    format!("messaging/tenant/{tenant_key}/{provider}/{name}")
}
