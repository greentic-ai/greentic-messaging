use axum::{http::StatusCode, response::IntoResponse, Json};
use serde_json::json;

/// Return a fast 202 ACK with a request id (if present)
pub fn ack202(request_id: Option<&String>) -> impl IntoResponse {
    let rid = request_id.cloned().unwrap_or_else(|| "n/a".to_string());
    (
        StatusCode::ACCEPTED,
        Json(json!({ "ok": true, "request_id": rid })),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::response::IntoResponse;
    use http_body_util::BodyExt;

    #[tokio::test]
    async fn ack202_uses_request_id_when_present() {
        let rid = "abc123".to_string();
        let response = ack202(Some(&rid)).into_response();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("collect")
            .to_bytes();
        let payload: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(payload, json!({ "ok": true, "request_id": "abc123" }));
    }

    #[tokio::test]
    async fn ack202_defaults_when_missing() {
        let response = ack202(None).into_response();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("collect")
            .to_bytes();
        let payload: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(payload, json!({ "ok": true, "request_id": "n/a" }));
    }
}
