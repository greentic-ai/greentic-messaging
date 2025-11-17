use crate::admin::secrets;

#[test]
fn builds_global_key() {
    assert_eq!(
        secrets::global_key("teams", "client_id"),
        "messaging/global/teams/client_id"
    );
}

#[test]
fn builds_tenant_key() {
    assert_eq!(
        secrets::tenant_key("tenant1", "teams", "app_token"),
        "messaging/tenant/tenant1/teams/app_token"
    );
}
