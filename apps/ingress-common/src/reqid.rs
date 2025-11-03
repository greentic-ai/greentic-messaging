use axum::{
    body::Body,
    http::{HeaderValue, Request, header::HeaderName},
    middleware::Next,
    response::Response,
};
use uuid::Uuid;

pub async fn with_request_id(mut req: Request<Body>, next: Next) -> Response {
    let rid = Uuid::new_v4().to_string();
    req.extensions_mut().insert(rid.clone());

    let mut res = next.run(req).await;
    if let Ok(value) = HeaderValue::from_str(&rid) {
        res.headers_mut()
            .insert(HeaderName::from_static("x-request-id"), value);
    }
    res
}
