use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
    time::Instant,
};

use axum::{
    body::Body,
    http::{Request, StatusCode},
    response::{IntoResponse, Response},
};
use tower::{Layer, Service, util::ServiceExt};

#[derive(Clone)]
pub struct RateLimiter {
    inner: Arc<Mutex<HashMap<String, (u32, Instant)>>>,
    cap: u32,
    refill_per_sec: u32,
}

impl RateLimiter {
    pub fn new(cap: u32, refill_per_sec: u32) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            cap,
            refill_per_sec,
        }
    }

    fn refill(entry: &mut (u32, Instant), cap: u32, refill_per_sec: u32) {
        let now = Instant::now();
        let elapsed = now.duration_since(entry.1);
        let refill = (elapsed.as_secs_f64() * refill_per_sec as f64).floor() as u32;
        if refill > 0 {
            entry.0 = entry.0.saturating_add(refill).min(cap);
            entry.1 = now;
        }
    }

    pub fn check(&self, key: &str) -> bool {
        let mut map = self.inner.lock().unwrap();
        let entry = map
            .entry(key.to_string())
            .or_insert((self.cap, Instant::now()));
        Self::refill(entry, self.cap, self.refill_per_sec);
        if entry.0 > 0 {
            entry.0 -= 1;
            true
        } else {
            false
        }
    }
}

#[derive(Clone)]
pub struct RateLimitLayer {
    limiter: RateLimiter,
}

impl RateLimitLayer {
    pub fn new(cap: u32, refill_per_sec: u32) -> Self {
        Self {
            limiter: RateLimiter::new(cap, refill_per_sec),
        }
    }
}

impl<S> Layer<S> for RateLimitLayer
where
    S: Service<Request<Body>, Response = Response> + Clone,
    S::Error: Send + 'static,
    S::Future: Send + 'static,
{
    type Service = RateLimitMiddleware<S>;

    fn layer(&self, inner: S) -> Self::Service {
        RateLimitMiddleware {
            inner,
            limiter: self.limiter.clone(),
        }
    }
}

#[derive(Clone)]
pub struct RateLimitMiddleware<S> {
    inner: S,
    limiter: RateLimiter,
}

impl<S> Service<Request<Body>> for RateLimitMiddleware<S>
where
    S: Service<Request<Body>, Response = Response> + Clone + Send + 'static,
    S::Error: Send + 'static,
    S::Future: Send + 'static,
{
    type Response = Response;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        let key = req
            .headers()
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("unknown")
            .to_string();
        let allow = self.limiter.check(&key);
        let mut inner = self.inner.clone();
        Box::pin(async move {
            if allow {
                inner.ready().await?.call(req).await
            } else {
                Ok(StatusCode::TOO_MANY_REQUESTS.into_response())
            }
        })
    }
}

pub fn rate_limit_layer(cap: u32, refill_per_sec: u32) -> RateLimitLayer {
    RateLimitLayer::new(cap, refill_per_sec)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::service_fn;

    #[test]
    fn limiter_refills_after_wait() {
        let limiter = RateLimiter::new(1, 10);
        assert!(limiter.check("client"));
        assert!(!limiter.check("client"));
        std::thread::sleep(std::time::Duration::from_millis(120));
        assert!(limiter.check("client"));
    }

    #[test]
    fn limiter_isolated_per_key() {
        let limiter = RateLimiter::new(1, 1);
        assert!(limiter.check("a"));
        assert!(limiter.check("b"));
        assert!(!limiter.check("a"));
        assert!(!limiter.check("b"));
    }

    #[tokio::test]
    async fn middleware_returns_429_when_exceeded() {
        let base = service_fn(|_req: Request<Body>| async {
            Ok::<_, std::convert::Infallible>(axum::http::Response::new(Body::empty()))
        });
        let layer = rate_limit_layer(1, 0);
        let mut svc = layer.layer(base);
        let request = || {
            Request::builder()
                .uri("/")
                .header("x-forwarded-for", "1.2.3.4")
                .body(Body::empty())
                .unwrap()
        };

        let first = svc.ready().await.unwrap().call(request()).await.unwrap();
        assert_eq!(first.status(), axum::http::StatusCode::OK);

        let second = svc.ready().await.unwrap().call(request()).await.unwrap();
        assert_eq!(second.status(), axum::http::StatusCode::TOO_MANY_REQUESTS);
    }
}
