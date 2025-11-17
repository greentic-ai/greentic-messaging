use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use async_trait::async_trait;
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use tower::ServiceExt;
use url::Url;

use crate::admin::{
    AdminRegistry,
    models::{DesiredTenantBinding, ProvisionCaps, ProvisionReport},
    plan::{ProvisionReportBuilder, ReportMode},
    router::admin_router,
    traits::{AdminResult, GlobalProvisioner, TenantProvisioner},
};

const BODY_LIMIT: usize = 1024 * 1024;

#[tokio::test]
async fn providers_endpoint_lists_caps() {
    let (app, _) = test_router();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/providers")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), BODY_LIMIT).await.unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload.as_array().unwrap().len(), 1);
    assert_eq!(payload[0]["provider"], "mock");
    assert_eq!(payload[0]["caps"]["model"], "global+tenant");
}

#[tokio::test]
async fn tenant_plan_sets_mode() {
    let (app, _) = test_router();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mock/tenant/plan")
                .header("content-type", "application/json")
                .body(Body::from(sample_binding_json()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), BODY_LIMIT).await.unwrap();
    let report: ProvisionReport = serde_json::from_slice(&body).unwrap();
    assert_eq!(report.mode.as_deref(), Some("plan"));
    assert_eq!(report.created.len(), 1);
}

#[tokio::test]
async fn tenant_ensure_is_idempotent() {
    let (app, _) = test_router();
    let req = || {
        Request::builder()
            .method("POST")
            .uri("/mock/tenant/ensure")
            .header("content-type", "application/json")
            .body(Body::from(sample_binding_json()))
            .unwrap()
    };

    let first = app.clone().oneshot(req()).await.unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let body = to_bytes(first.into_body(), BODY_LIMIT).await.unwrap();
    let report: ProvisionReport = serde_json::from_slice(&body).unwrap();
    assert_eq!(report.created.len(), 1);
    assert!(report.skipped.is_empty());

    let second = app.clone().oneshot(req()).await.unwrap();
    let body = to_bytes(second.into_body(), BODY_LIMIT).await.unwrap();
    let report: ProvisionReport = serde_json::from_slice(&body).unwrap();
    assert!(report.created.is_empty());
    assert_eq!(report.skipped.len(), 1);
}

#[tokio::test]
async fn global_ensure_invokes_provider() {
    let (app, provider) = test_router();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mock/global/ensure")
                .header("content-type", "application/json")
                .body(Body::from(sample_global_app_json()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), BODY_LIMIT).await.unwrap();
    let report: ProvisionReport = serde_json::from_slice(&body).unwrap();
    assert_eq!(report.mode.as_deref(), Some("ensure"));

    let calls = provider.global_calls.lock().await;
    assert_eq!(*calls, 1);
}

#[tokio::test]
async fn tenant_start_and_callback_flow() {
    let (app, provider) = test_router();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mock/tenant/start?tenant_key=tenant1&provider_tenant_id=mock_tenant")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), BODY_LIMIT).await.unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(payload["started"].as_bool().unwrap());
    assert!(
        payload["redirect_url"]
            .as_str()
            .unwrap()
            .contains("consent")
    );

    let callback = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/mock/tenant/callback?tenant_key=tenant1&code=abc")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(callback.status(), StatusCode::OK);

    let callbacks = provider.tenant_callbacks.lock().await;
    assert!(callbacks.contains(&"tenant1".to_string()));
}

fn sample_binding_json() -> String {
    serde_json::json!({
        "tenant_key": "tenant1",
        "provider_tenant_id": "mock_tenant",
        "requested_scopes": ["chat:write"],
        "resources": [{
            "kind": "team",
            "id": "team1",
            "display_name": "Support"
        }],
        "credential_policy": {"ClientSecret": {"rotate_days": 90}},
        "extra_params": null
    })
    .to_string()
}

fn sample_global_app_json() -> String {
    serde_json::json!({
        "display_name": "Mock App",
        "redirect_uris": ["https://example.com/callback"],
        "allowed_scopes": ["chat:write"],
        "capabilities": ["bot"],
        "extra_params": null
    })
    .to_string()
}

fn test_router() -> (axum::Router, Arc<MockProvisioner>) {
    let caps = ProvisionCaps {
        model: "global+tenant".into(),
        requires_global_bootstrap: true,
        supports_rsc: true,
        supports_webhooks: true,
        supports_per_resource_install: true,
    };
    let provider = Arc::new(MockProvisioner::new(caps));

    let mut globals: HashMap<&'static str, Arc<dyn GlobalProvisioner>> = HashMap::new();
    let global_impl: Arc<dyn GlobalProvisioner> = provider.clone();
    globals.insert("mock", global_impl);
    let mut tenants: HashMap<&'static str, Arc<dyn TenantProvisioner>> = HashMap::new();
    let tenant_impl: Arc<dyn TenantProvisioner> = provider.clone();
    tenants.insert("mock", tenant_impl);

    let registry = AdminRegistry::new(globals, tenants);
    (admin_router(Arc::new(registry)), provider)
}

#[derive(Clone)]
struct MockProvisioner {
    caps: ProvisionCaps,
    seen_tenants: Arc<tokio::sync::Mutex<HashSet<String>>>,
    global_calls: Arc<tokio::sync::Mutex<u32>>,
    tenant_callbacks: Arc<tokio::sync::Mutex<Vec<String>>>,
    consent_url: Url,
}

impl MockProvisioner {
    fn new(caps: ProvisionCaps) -> Self {
        Self {
            caps,
            seen_tenants: Arc::new(tokio::sync::Mutex::new(HashSet::new())),
            global_calls: Arc::new(tokio::sync::Mutex::new(0)),
            tenant_callbacks: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            consent_url: Url::parse("https://example.com/consent").unwrap(),
        }
    }

    fn provider_name(&self) -> &'static str {
        "mock"
    }
}

#[async_trait]
impl GlobalProvisioner for MockProvisioner {
    fn provider(&self) -> &'static str {
        self.provider_name()
    }

    fn capabilities(&self) -> ProvisionCaps {
        self.caps.clone()
    }

    async fn ensure_global(
        &self,
        desired: &crate::admin::models::DesiredGlobalApp,
    ) -> AdminResult<ProvisionReport> {
        let mut builder =
            ProvisionReportBuilder::new(self.provider_name(), None, ReportMode::Ensure);
        builder.created(format!("app:{}", desired.display_name));
        let mut calls = self.global_calls.lock().await;
        *calls += 1;
        Ok(builder.finish())
    }

    async fn start_global_consent(&self) -> AdminResult<Option<Url>> {
        Ok(Some(self.consent_url.clone()))
    }
}

#[async_trait]
impl TenantProvisioner for MockProvisioner {
    fn provider(&self) -> &'static str {
        self.provider_name()
    }

    async fn start_tenant_consent(
        &self,
        _tenant_key: &str,
        _provider_tenant_id: &str,
    ) -> AdminResult<Option<Url>> {
        Ok(Some(self.consent_url.clone()))
    }

    async fn handle_tenant_callback(
        &self,
        tenant_key: &str,
        _query: &[(String, String)],
    ) -> AdminResult<()> {
        let mut callbacks = self.tenant_callbacks.lock().await;
        callbacks.push(tenant_key.to_string());
        Ok(())
    }

    async fn ensure_tenant(&self, desired: &DesiredTenantBinding) -> AdminResult<ProvisionReport> {
        let mut builder = ProvisionReportBuilder::new(
            self.provider_name(),
            Some(&desired.tenant_key),
            ReportMode::Ensure,
        );
        let mut seen = self.seen_tenants.lock().await;
        if seen.insert(desired.tenant_key.clone()) {
            builder.created(format!("binding:{}", desired.tenant_key));
        } else {
            builder.skipped(format!("binding:{}", desired.tenant_key));
        }
        Ok(builder.finish())
    }

    async fn plan_tenant(&self, desired: &DesiredTenantBinding) -> AdminResult<ProvisionReport> {
        let mut builder = ProvisionReportBuilder::new(
            self.provider_name(),
            Some(&desired.tenant_key),
            ReportMode::Plan,
        );
        builder.created(format!("binding:{}", desired.tenant_key));
        Ok(builder.finish())
    }
}
