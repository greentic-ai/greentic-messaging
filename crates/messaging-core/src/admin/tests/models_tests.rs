use std::collections::BTreeMap;
use url::Url;

use crate::admin::{
    models::{
        CredentialPolicy, DesiredGlobalApp, DesiredTenantBinding, ProvisionCaps, ProvisionReport,
        ResourceSpec,
    },
    plan::{validate_global_app, validate_resource, validate_tenant_binding},
    traits::AdminError,
};

#[test]
fn desired_global_app_roundtrip() {
    let app = sample_global_app();
    let json = serde_json::to_string(&app).unwrap();
    let de: DesiredGlobalApp = serde_json::from_str(&json).unwrap();
    assert_eq!(app, de);
}

#[test]
fn desired_tenant_binding_roundtrip() {
    let binding = sample_tenant_binding();
    let json = serde_json::to_string(&binding).unwrap();
    let de: DesiredTenantBinding = serde_json::from_str(&json).unwrap();
    assert_eq!(binding, de);
}

#[test]
fn resource_spec_roundtrip() {
    let resource = ResourceSpec {
        kind: "team".into(),
        id: "123".into(),
        display_name: Some("Example".into()),
    };
    let json = serde_json::to_string(&resource).unwrap();
    let de: ResourceSpec = serde_json::from_str(&json).unwrap();
    assert_eq!(resource, de);
}

#[test]
fn provision_caps_roundtrip() {
    let caps = ProvisionCaps {
        model: "global+tenant".into(),
        requires_global_bootstrap: true,
        supports_rsc: true,
        supports_webhooks: true,
        supports_per_resource_install: false,
    };
    let json = serde_json::to_string(&caps).unwrap();
    let de: ProvisionCaps = serde_json::from_str(&json).unwrap();
    assert_eq!(caps, de);
}

#[test]
fn provision_report_roundtrip() {
    let report = ProvisionReport {
        provider: "mock".into(),
        tenant: Some("tenant1".into()),
        created: vec!["app".into()],
        updated: vec![],
        skipped: vec![],
        warnings: vec!["warn".into()],
        secret_keys_written: vec!["messaging/global/mock/key".into()],
        mode: Some("ensure".into()),
    };
    let json = serde_json::to_string(&report).unwrap();
    let de: ProvisionReport = serde_json::from_str(&json).unwrap();
    assert_eq!(report, de);
}

#[test]
fn rejects_invalid_extra_param_value() {
    let mut binding = sample_tenant_binding();
    binding.extra_params = Some(BTreeMap::from([(
        String::from("bad"),
        String::from("\u{0001}"),
    )]));
    assert!(matches!(
        validate_tenant_binding(&binding),
        Err(AdminError::Validation(_))
    ));
}

#[test]
fn rejects_long_extra_param_value() {
    let mut binding = sample_tenant_binding();
    let long = "x".repeat(300);
    binding.extra_params = Some(BTreeMap::from([(String::from("long"), long)]));
    assert!(matches!(
        validate_tenant_binding(&binding),
        Err(AdminError::Validation(_))
    ));
}

#[test]
fn rejects_empty_tenant_key() {
    let mut binding = sample_tenant_binding();
    binding.tenant_key.clear();
    assert!(matches!(
        validate_tenant_binding(&binding),
        Err(AdminError::Validation(_))
    ));
}

#[test]
fn rejects_global_display_name() {
    let mut app = sample_global_app();
    app.display_name.clear();
    assert!(matches!(
        validate_global_app(&app),
        Err(AdminError::Validation(_))
    ));
}

#[test]
fn rejects_resource_id_control_char() {
    let resource = ResourceSpec {
        kind: "team".into(),
        id: "bad\u{0003}".into(),
        display_name: None,
    };
    assert!(matches!(
        validate_resource(&resource),
        Err(AdminError::Validation(_))
    ));
}

fn sample_global_app() -> DesiredGlobalApp {
    DesiredGlobalApp {
        display_name: "Mock App".into(),
        redirect_uris: vec![Url::parse("https://example.com/cb").unwrap()],
        allowed_scopes: vec!["chat:write".into()],
        capabilities: vec!["bot".into()],
        extra_params: Some(BTreeMap::from([(String::from("env"), String::from("dev"))])),
    }
}

fn sample_tenant_binding() -> DesiredTenantBinding {
    DesiredTenantBinding {
        tenant_key: "tenant1".into(),
        provider_tenant_id: "T1".into(),
        requested_scopes: vec!["chat:write".into()],
        resources: vec![ResourceSpec {
            kind: "team".into(),
            id: "abc".into(),
            display_name: Some("Support".into()),
        }],
        credential_policy: CredentialPolicy::ClientSecret { rotate_days: 90 },
        extra_params: None,
    }
}
