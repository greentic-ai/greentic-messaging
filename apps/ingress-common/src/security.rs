use anyhow::Result;
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header::HeaderName},
    middleware::Next,
    response::{IntoResponse, Response},
};
use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::str::FromStr;

#[derive(Clone, Default)]
pub struct IngressSecurityConfig {
    pub hmac_secret: Option<String>,
    pub hmac_header: Option<String>,
    pub bearer: Option<String>,
}

/// Shared secret HMAC check (header name + body signature verification)
pub async fn verify_hmac(req: Request<Body>, next: Next) -> Response {
    let cfg = req
        .extensions()
        .get::<IngressSecurityConfig>()
        .cloned()
        .unwrap_or_default();
    let secret = cfg.hmac_secret;
    let header_name = cfg.hmac_header.unwrap_or_else(|| "x-signature".into());

    if let Some(secret) = secret {
        let header_name =
            HeaderName::from_str(&header_name).unwrap_or(HeaderName::from_static("x-signature"));
        let (parts, body) = req.into_parts();
        let body_bytes = match to_bytes(body, usize::MAX).await {
            Ok(bytes) => bytes,
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        };
        let provided_sig = parts
            .headers
            .get(&header_name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if provided_sig.is_empty() {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        if hmac_verify(&secret, &body_bytes, provided_sig).is_err() {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        let req = Request::from_parts(parts, Body::from(body_bytes));
        next.run(req).await
    } else {
        next.run(req).await
    }
}

fn hmac_verify(secret: &str, body: &[u8], sig_hdr: &str) -> Result<()> {
    type HmacSha256 = Hmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())?;
    mac.update(body);
    let provided = B64.decode(sig_hdr)?;
    mac.verify_slice(&provided)
        .map_err(|_| anyhow::anyhow!("bad signature"))
}

/// Simple bearer token check on `Authorization: Bearer <TOKEN>`
pub async fn verify_bearer(req: Request<Body>, next: Next) -> Response {
    let cfg = req
        .extensions()
        .get::<IngressSecurityConfig>()
        .cloned()
        .unwrap_or_default();
    if let Some(token) = cfg.bearer {
        let ok = req
            .headers()
            .get("authorization")
            .and_then(|h| h.to_str().ok())
            .map(|s| s == format!("Bearer {token}"))
            .unwrap_or(false);
        if !ok {
            return StatusCode::UNAUTHORIZED.into_response();
        }
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Extension, Router, body::Body, middleware, routing::get};
    use base64::Engine;
    use hmac::Mac;
    use rand::RngCore;
    use tower::ServiceExt;

    fn random_secret() -> String {
        let mut buf = [0u8; 32];
        let mut rng = rand::rng();
        rng.fill_bytes(&mut buf);
        base64::engine::general_purpose::STANDARD.encode(buf)
    }

    #[test]
    fn hmac_verify_accepts_valid_signature() {
        let secret = random_secret();
        let body = br#"{"ok":true}"#;
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        let sig = base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());
        assert!(hmac_verify(&secret, body, &sig).is_ok());
    }

    #[tokio::test]
    async fn verify_bearer_blocks_invalid_token() {
        let app = Router::new()
            .route("/", get(|| async { axum::http::StatusCode::OK }))
            .layer(middleware::from_fn(verify_bearer))
            .layer(Extension(IngressSecurityConfig {
                bearer: Some("expected".into()),
                ..Default::default()
            }));

        let req = axum::http::Request::builder()
            .uri("/")
            .body(Body::empty())
            .unwrap();
        let resp: axum::response::Response = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::UNAUTHORIZED);

        let ok_req = axum::http::Request::builder()
            .uri("/")
            .header("authorization", "Bearer expected")
            .body(Body::empty())
            .unwrap();
        let ok_resp: axum::response::Response = app.oneshot(ok_req).await.unwrap();
        assert_eq!(ok_resp.status(), axum::http::StatusCode::OK);
    }

    #[tokio::test]
    async fn verify_bearer_allows_missing_config() {
        let app = Router::new()
            .route("/", get(|| async { axum::http::StatusCode::OK }))
            .layer(middleware::from_fn(verify_bearer));

        let req = axum::http::Request::builder()
            .uri("/")
            .body(Body::empty())
            .unwrap();
        let resp: axum::response::Response = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
    }

    #[tokio::test]
    async fn verify_hmac_allows_missing_config() {
        let app = Router::new()
            .route("/", get(|| async { axum::http::StatusCode::OK }))
            .layer(middleware::from_fn(verify_hmac));

        let req = axum::http::Request::builder()
            .uri("/")
            .body(Body::from("payload"))
            .unwrap();
        let resp: axum::response::Response = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
    }

    #[tokio::test]
    async fn verify_hmac_rejects_bad_signature() {
        let secret = random_secret();
        let app = Router::new()
            .route("/", get(|| async { axum::http::StatusCode::OK }))
            .layer(middleware::from_fn(verify_hmac))
            .layer(Extension(IngressSecurityConfig {
                hmac_secret: Some(secret.clone()),
                hmac_header: Some("x-signature".into()),
                ..Default::default()
            }));

        let req = axum::http::Request::builder()
            .uri("/")
            .body(Body::from("payload"))
            .unwrap();
        let resp: axum::response::Response = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::UNAUTHORIZED);

        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(b"payload");
        let sig = base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());
        let ok_req = axum::http::Request::builder()
            .uri("/")
            .header("x-signature", sig)
            .body(Body::from("payload"))
            .unwrap();
        let ok_resp: axum::response::Response = app.oneshot(ok_req).await.unwrap();
        assert_eq!(ok_resp.status(), axum::http::StatusCode::OK);
    }
}
