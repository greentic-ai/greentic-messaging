use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use greentic_messaging_providers_webchat::config::Config;
use greentic_messaging_providers_webchat::{StandaloneState, standalone_router};
use std::sync::Arc;
use tower::ServiceExt;

#[tokio::test]
async fn standalone_generate_token_round_trip() {
    // Ensure deterministic config for the standalone signing key.
    unsafe {
        std::env::set_var("WEBCHAT_JWT_SIGNING_KEY", "test-signing-key");
    }

    let state = Arc::new(
        StandaloneState::new(Config::with_base_url("http://localhost"))
            .await
            .unwrap(),
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
