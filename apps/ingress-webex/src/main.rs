use anyhow::Result;
use async_nats::Client as NatsClient;
use async_trait::async_trait;
use axum::{
    body::Bytes,
    extract::State,
    http::{header::HeaderName, HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Router,
};
use gsm_core::in_subject;
use gsm_idempotency::{IdKey, IdempotencyGuard};
use gsm_ingress_common::{
    init_guard, record_idempotency_hit, record_ingress, signature_header_from_env,
    start_ingress_span, SignatureAlgorithm,
};
use gsm_telemetry::{init_telemetry, TelemetryConfig};
use serde_json::Value;
use std::{net::SocketAddr, str::FromStr, sync::Arc};
use tracing::{error, info, warn};

mod normalise;
mod verify;

use normalise::normalise_webhook;
use verify::verify_signature;

type SharedPublisher = Arc<dyn Publisher>;

#[async_trait]
pub trait Publisher: Send + Sync {
    async fn publish(&self, subject: &str, payload: Vec<u8>) -> anyhow::Result<()>;
}

#[derive(Clone)]
struct NatsPublisher {
    client: NatsClient,
}

#[async_trait]
impl Publisher for NatsPublisher {
    async fn publish(&self, subject: &str, payload: Vec<u8>) -> anyhow::Result<()> {
        self.client
            .publish(subject.to_string(), payload.into())
            .await?;
        Ok(())
    }
}

#[derive(Clone)]
struct AppState {
    tenant: String,
    secret: String,
    signature_header: HeaderName,
    signature_algorithm: SignatureAlgorithm,
    guard: IdempotencyGuard,
    publisher: SharedPublisher,
}

fn router(state: AppState) -> Router {
    Router::new()
        .route("/webex/messages", post(handle_webex))
        .route("/healthz", get(healthz))
        .with_state(state)
}

#[tokio::main]
async fn main() -> Result<()> {
    let telemetry = TelemetryConfig::from_env("gsm-ingress-webex", env!("CARGO_PKG_VERSION"));
    init_telemetry(telemetry)?;

    let nats_url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
    let tenant = std::env::var("TENANT").unwrap_or_else(|_| "acme".into());
    let secret = std::env::var("WEBEX_WEBHOOK_SECRET")
        .expect("WEBEX_WEBHOOK_SECRET environment variable required");
    let header_name = signature_header_from_env();
    let signature_header =
        HeaderName::from_str(&header_name).expect("invalid header name in WEBEX_SIG_HEADER");
    let signature_algorithm = SignatureAlgorithm::from_env();

    let nats = async_nats::connect(nats_url).await?;
    let guard = init_guard(&nats).await?;
    let publisher: SharedPublisher = Arc::new(NatsPublisher { client: nats });

    let state = AppState {
        tenant,
        secret,
        signature_header,
        signature_algorithm,
        guard,
        publisher,
    };

    let addr: SocketAddr = std::env::var("BIND")
        .unwrap_or_else(|_| "0.0.0.0:8088".into())
        .parse()
        .expect("invalid BIND address");

    info!("ingress-webex listening on {addr}");
    let app = router(state);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app.into_make_service()).await?;
    Ok(())
}

async fn healthz() -> impl IntoResponse {
    StatusCode::NO_CONTENT
}

async fn handle_webex(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    match process_webhook(state, headers, body).await {
        Ok(status) => status,
        Err(status) => status,
    }
}

async fn process_webhook(
    state: AppState,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, StatusCode> {
    let signature_value = headers
        .get(&state.signature_header)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| {
            warn!("missing webex signature header");
            StatusCode::UNAUTHORIZED
        })?;

    let verified = verify_signature(
        &state.secret,
        signature_value,
        &body,
        state.signature_algorithm,
    )
    .map_err(|err| {
        error!(error = %err, "failed to verify webex signature");
        StatusCode::UNAUTHORIZED
    })?;

    if !verified {
        warn!("invalid webex signature");
        return Err(StatusCode::UNAUTHORIZED);
    }

    let raw: Value = serde_json::from_slice(&body).map_err(|err| {
        error!(error = %err, "failed to decode webex webhook");
        StatusCode::BAD_REQUEST
    })?;

    let envelope = normalise_webhook(&state.tenant, &raw).map_err(|err| {
        error!(error = %err, "failed to normalise webex payload");
        StatusCode::BAD_REQUEST
    })?;

    let span = start_ingress_span(&envelope);
    let _guard = span.enter();

    let key = IdKey {
        tenant: envelope.tenant.clone(),
        platform: envelope.platform.as_str().to_string(),
        msg_id: envelope.msg_id.clone(),
    };

    match state.guard.should_process(&key).await {
        Ok(true) => {}
        Ok(false) => {
            record_idempotency_hit(&key.tenant);
            info!(
                tenant = %key.tenant,
                platform = %key.platform,
                msg_id = %key.msg_id,
                "duplicate webex event dropped"
            );
            return Ok(StatusCode::OK);
        }
        Err(err) => {
            error!(error = %err, tenant = %key.tenant, "idempotency check failed");
        }
    }

    let subject = in_subject(&state.tenant, envelope.platform.as_str(), &envelope.chat_id);
    let payload = serde_json::to_vec(&envelope).map_err(|err| {
        error!(error = %err, "failed to serialise envelope");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    state
        .publisher
        .publish(&subject, payload)
        .await
        .map_err(|err| {
            error!(error = %err, subject = %subject, "failed to publish to nats");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    record_ingress(&envelope);
    info!(chat_id = %envelope.chat_id, "webex message published");

    Ok(StatusCode::OK)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use gsm_core::{MessageEnvelope, Platform};
    use gsm_idempotency::{InMemoryIdemStore, SharedIdemStore};
    use std::sync::Mutex;
    use tower::ServiceExt;

    #[derive(Clone, Default)]
    struct MockPublisher {
        events: Arc<Mutex<Vec<(String, Vec<u8>)>>>,
    }

    #[async_trait]
    impl Publisher for MockPublisher {
        async fn publish(&self, subject: &str, payload: Vec<u8>) -> anyhow::Result<()> {
            self.events
                .lock()
                .unwrap()
                .push((subject.to_string(), payload));
            Ok(())
        }
    }

    fn build_state() -> (AppState, Arc<MockPublisher>) {
        let tenant = "acme".to_string();
        let secret = "top-secret".to_string();
        let signature_header = HeaderName::from_static("x-webex-signature");
        let signature_algorithm = SignatureAlgorithm::Sha1;
        let store: SharedIdemStore = Arc::new(InMemoryIdemStore::new());
        let guard = IdempotencyGuard::new(store, 1);
        let mock = Arc::new(MockPublisher::default());
        let publisher: SharedPublisher = mock.clone();

        (
            AppState {
                tenant,
                secret,
                signature_header,
                signature_algorithm,
                guard,
                publisher,
            },
            mock,
        )
    }

    fn sign(secret: &str, body: &[u8]) -> String {
        let sig = super::verify::compute_signature(secret, body, SignatureAlgorithm::Sha1)
            .expect("signature");
        format!("sha1={}", hex::encode(sig))
    }

    fn build_request(body: &str, signature: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri("/webex/messages")
            .header("content-type", "application/json")
            .header("x-webex-signature", signature)
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    #[tokio::test]
    async fn publishes_and_dedupes() {
        let body = r#"{
            "resource": "messages",
            "event": "created",
            "data": {
                "id": "mid-001",
                "roomId": "room-9",
                "personId": "person-7",
                "created": "2024-01-01T00:00:00Z",
                "text": "hi"
            }
        }"#;

        let (state, publisher) = build_state();
        let app = router(state.clone());

        let signature = sign(&state.secret, body.as_bytes());
        let req = build_request(body, &signature);
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        let req2 = build_request(body, &signature);
        let res2 = app.oneshot(req2).await.unwrap();
        assert_eq!(res2.status(), StatusCode::OK);

        let stored = publisher.events.lock().unwrap().clone();
        assert_eq!(stored.len(), 1, "duplicate payload should not republish");

        let (subject, payload) = &stored[0];
        assert_eq!(subject, "greentic.msg.in.acme.webex.room-9");
        let env: MessageEnvelope = serde_json::from_slice(payload).unwrap();
        assert_eq!(env.platform, Platform::Webex);
        assert_eq!(env.msg_id, "mid-001");
    }
}
