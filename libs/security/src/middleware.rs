use std::sync::Arc;

use anyhow::Result;
use axum::{
    extract::FromRequestParts,
    http::{header, request::Parts, StatusCode},
    response::{Html, IntoResponse, Redirect, Response},
    routing::get,
    Router,
};
use time::OffsetDateTime;
use tracing::{debug, warn};

use crate::{
    jwt::{ActionClaims, JwtSigner},
    nonce::{default_nonce_store, SharedNonceStore},
};

const FAILURE_HTML: &str = r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><title>Link expired</title><style>body{font-family:-apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif;background:#0f172a;color:#e2e8f0;display:flex;align-items:center;justify-content:center;height:100vh;margin:0;}main{max-width:420px;text-align:center;background:rgba(15,23,42,0.8);padding:2.5rem 2rem;border-radius:1rem;box-shadow:0 25px 50px -12px rgba(15,23,42,0.5);}h1{font-size:1.5rem;margin-bottom:0.75rem;}p{color:#cbd5f5;font-size:0.95rem;line-height:1.6;}</style></head><body><main><h1>This link is invalid or expired</h1><p>Please go back to your conversation and request a new link. This safeguard keeps actions secure even if a link is forwarded.</p></main></body></html>"#;
const SUCCESS_HTML: &str = r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><title>Action received</title><style>body{font-family:-apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif;background:#f8fafc;color:#0f172a;display:flex;align-items:center;justify-content:center;height:100vh;margin:0;}main{max-width:420px;text-align:center;background:#fff;padding:2.5rem 2rem;border-radius:1rem;box-shadow:0 15px 35px rgba(15,23,42,0.15);}h1{font-size:1.5rem;margin-bottom:0.75rem;}p{color:#475569;font-size:0.95rem;line-height:1.6;}</style></head><body><main><h1>Thanks! We received your response.</h1><p>If the conversation needs anything else, we’ll let you know in the original channel.</p></main></body></html>"#;

#[derive(Clone)]
pub struct ActionContext {
    signer: Arc<JwtSigner>,
    store: SharedNonceStore,
}

impl ActionContext {
    pub fn new(signer: JwtSigner, store: SharedNonceStore) -> Self {
        Self {
            signer: Arc::new(signer),
            store,
        }
    }

    pub async fn from_env(client: &async_nats::Client) -> Result<Self> {
        let signer = JwtSigner::from_env()?;
        let store: SharedNonceStore = Arc::new(default_nonce_store(client).await?);
        Ok(Self::new(signer, store))
    }

    pub fn signer(&self) -> &JwtSigner {
        &self.signer
    }

    pub async fn consume(&self, claims: &ActionClaims) -> Result<bool> {
        let ttl = claims.ttl_seconds();
        self.store
            .consume(&claims.tenant, &claims.jti, &claims.nonce, ttl)
            .await
    }
}

pub type SharedActionContext = Arc<ActionContext>;

pub fn action_router() -> Router {
    Router::new()
        .route("/a", get(handle_action))
        .route("/a/{_platform}", get(handle_action))
}

pub struct ActionAuth(pub ActionClaims);

impl<S> FromRequestParts<S> for ActionAuth
where
    S: Send + Sync,
{
    type Rejection = Response;

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl std::future::Future<Output = Result<Self, Self::Rejection>> + Send {
        let ctx_opt = parts
            .extensions
            .get::<SharedActionContext>()
            .cloned()
            .or_else(|| {
                parts
                    .extensions
                    .get::<ActionContext>()
                    .map(|ctx| Arc::new(ctx.clone()))
            });
        let token = parts
            .uri
            .query()
            .and_then(extract_action_token)
            .map(|t| t.to_string());

        async move {
            let ctx = ctx_opt.ok_or_else(invalid_link)?;
            let token = token.ok_or_else(invalid_link)?;

            let decoded = match ctx.signer.verify(&token) {
                Ok(claims) => claims,
                Err(err) => {
                    debug!("action token verify failed: {err}");
                    return Err(invalid_link());
                }
            };

            let now = OffsetDateTime::now_utc().unix_timestamp();
            if decoded.exp < now {
                debug!("action token expired: exp={} now={}", decoded.exp, now);
                return Err(invalid_link());
            }

            if decoded.tenant.trim().is_empty() || decoded.scope.trim().is_empty() {
                debug!("action token missing tenant/scope");
                return Err(invalid_link());
            }

            match ctx.consume(&decoded).await {
                Ok(true) => Ok(ActionAuth(decoded)),
                Ok(false) => {
                    debug!("action token reused");
                    Err(invalid_link())
                }
                Err(err) => {
                    warn!("nonce consume failed: {err}");
                    Err(server_error())
                }
            }
        }
    }
}

pub async fn handle_action(ActionAuth(claims): ActionAuth) -> Response {
    if let Some(target) = claims.redirect {
        Redirect::to(&target).into_response()
    } else {
        (StatusCode::OK, Html(SUCCESS_HTML)).into_response()
    }
}

fn extract_action_token(query: &str) -> Option<String> {
    for pair in query.split('&') {
        let mut parts = pair.splitn(2, '=');
        let key = parts.next()?;
        if key == "action" {
            let value = parts.next().unwrap_or("");
            return urlencoding::decode(value).ok().map(|cow| cow.into_owned());
        }
    }
    None
}

fn invalid_link() -> Response {
    let mut response = (StatusCode::GONE, Html(FAILURE_HTML)).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    response
}

fn server_error() -> Response {
    (StatusCode::INTERNAL_SERVER_ERROR, Html(FAILURE_HTML)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use axum::{body::Body, http::Request, Extension};
    use base64::engine::general_purpose::STANDARD_NO_PAD;
    use base64::Engine as _;
    use once_cell::sync::Lazy;
    use std::{collections::HashSet, sync::Mutex, time::Duration as StdDuration};
    use time::Duration;
    use tokio::sync::Mutex as AsyncMutex;
    use tower::util::ServiceExt;

    use crate::{hash::state_hash_parts, jwt::ActionClaims, links::action_ttl, nonce::NonceStore};

    static ENV_GUARD: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

    #[derive(Clone, Default)]
    struct MemoryStore {
        seen: Arc<AsyncMutex<HashSet<String>>>,
    }

    #[async_trait]
    #[async_trait]
    impl NonceStore for MemoryStore {
        async fn consume(
            &self,
            tenant: &str,
            jti: &str,
            _nonce: &str,
            _ttl_secs: u64,
        ) -> Result<bool> {
            let key = format!("{tenant}:{jti}");
            let mut guard = self.seen.lock().await;
            if guard.contains(&key) {
                Ok(false)
            } else {
                guard.insert(key);
                Ok(true)
            }
        }
    }

    fn setup_signer() -> JwtSigner {
        std::env::set_var("JWT_ALG", "HS256");
        std::env::set_var("JWT_SECRET", "integration-secret");
        JwtSigner::from_env().expect("signer")
    }

    fn teardown_env() {
        std::env::remove_var("JWT_SECRET");
        std::env::remove_var("JWT_ALG");
    }

    fn build_router(ctx: SharedActionContext) -> Router {
        action_router().layer(Extension(ctx))
    }

    fn action_claims(signer: &JwtSigner, redirect: Option<String>) -> (ActionClaims, String) {
        let ttl = action_ttl();
        let claims = ActionClaims::new(
            "chat-1",
            "tenant-1",
            "demo.scope",
            state_hash_parts("tenant-1", "slack", "C123", "msg-1"),
            redirect,
            ttl,
        );
        let token = signer.sign(&claims).expect("token");
        (claims, token)
    }

    #[tokio::test]
    async fn action_success_and_single_use() {
        let (router, token) = {
            let _guard = ENV_GUARD.lock().unwrap();
            let signer = setup_signer();
            let store: SharedNonceStore = Arc::new(MemoryStore::default());
            let ctx = Arc::new(ActionContext::new(signer.clone(), store));
            let router = build_router(ctx.clone());
            let (_claims, token) = action_claims(&signer, Some("https://example.com/ok".into()));
            (router, token)
        };

        let uri = format!("/a?action={}", urlencoding::encode(&token));
        let response = router
            .clone()
            .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);

        // Second attempt should fail due to nonce reuse.
        let response = router
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::GONE);
        let _guard = ENV_GUARD.lock().unwrap();
        teardown_env();
    }

    #[tokio::test]
    async fn action_expired_is_rejected() {
        let (router, token) = {
            let _guard = ENV_GUARD.lock().unwrap();
            let signer = setup_signer();
            let store: SharedNonceStore = Arc::new(MemoryStore::default());
            let ctx = Arc::new(ActionContext::new(signer.clone(), store));
            let router = build_router(ctx);
            let ttl = Duration::seconds(1);
            let claims = ActionClaims::new(
                "chat-2",
                "tenant-2",
                "demo.scope",
                state_hash_parts("tenant-2", "teams", "chat", "msg"),
                None,
                ttl,
            );
            let token = signer.sign(&claims).expect("token");
            (router, token)
        };
        let uri = format!("/a?action={}", urlencoding::encode(&token));
        tokio::time::sleep(StdDuration::from_secs(2)).await;
        let response = router
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::GONE);
        let _guard = ENV_GUARD.lock().unwrap();
        teardown_env();
    }

    #[tokio::test]
    async fn action_tamper_is_detected() {
        let (router, token) = {
            let _guard = ENV_GUARD.lock().unwrap();
            let signer = setup_signer();
            let store: SharedNonceStore = Arc::new(MemoryStore::default());
            let ctx = Arc::new(ActionContext::new(signer.clone(), store));
            let router = build_router(ctx);
            let (_claims, token) = action_claims(&signer, None);
            (router, token)
        };

        let mut segments: Vec<String> = token.split('.').map(|s| s.to_string()).collect();
        assert_eq!(segments.len(), 3);
        let payload = STANDARD_NO_PAD.decode(&segments[1]).expect("payload");
        let mut json: serde_json::Value = serde_json::from_slice(&payload).expect("json");
        json["state_hash"] = serde_json::Value::String("evil".into());
        let tampered_payload = STANDARD_NO_PAD.encode(serde_json::to_vec(&json).unwrap());
        segments[1] = tampered_payload;
        let tampered = segments.join(".");
        let uri = format!("/a?action={}", urlencoding::encode(&tampered));

        let response = router
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::GONE);
        let _guard = ENV_GUARD.lock().unwrap();
        teardown_env();
    }
}
