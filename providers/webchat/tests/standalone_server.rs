use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use greentic_messaging_providers_webchat::config::Config;
use greentic_messaging_providers_webchat::{StandaloneState, standalone_router};
use std::sync::Arc;
use tower::ServiceExt;

#[path = "../test_support/mod.rs"]
mod support;

use support::{provider_with_secrets, signing_scope};

#[tokio::test]
async fn standalone_generate_token_round_trip() {
    let provider = provider_with_secrets(
        Config::with_base_url("http://localhost"),
        signing_scope(),
        &[],
    );
    let state = Arc::new(
        StandaloneState::new(provider)
            .await
            .expect("standalone state"),
    );
    let app = standalone_router(Arc::clone(&state));

    let request = Request::builder()
        .method("POST")
        .uri("/v3/directline/tokens/generate?env=dev&tenant=acme")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"user":{"id":"user-42"}}"#))
        .unwrap();

    let response = app
        .clone()
        .oneshot(request)
        .await
        .expect("router handles request");

    assert!(response.status().is_success());
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(json.get("token").and_then(|value| value.as_str()).is_some());
}
