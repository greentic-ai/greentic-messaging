//! WebChat egress adapter that streams outbound `OutMessage`s to a simple SSE UI.
//!
//! ```text
//! Start the binary, then open `/events` to watch translated payloads
//! as they arrive from NATS.
//! ```

use anyhow::Result;
use async_nats::jetstream::AckKind;
use async_stream::stream;
use axum::{
    extract::State,
    response::{Html, IntoResponse},
    routing::get,
    Router,
};
use futures::StreamExt;
use gsm_backpressure::BackpressureLimiter;
use gsm_core::{OutMessage, Platform};
use gsm_dlq::{DlqError, DlqPublisher};
use gsm_egress_common::egress::bootstrap;
use gsm_translator::{Translator, WebChatTranslator};
use include_dir::{include_dir, Dir};
use serde_json::Value;
use tokio::sync::broadcast;
use tracing::{event, Instrument, Level};
use tracing_subscriber::EnvFilter;

static ASSETS: Dir = include_dir!("$CARGO_MANIFEST_DIR/static");

#[derive(Clone)]
struct AppState {
    latest: broadcast::Sender<Value>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let nats_url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
    let tenant = std::env::var("TENANT").unwrap_or_else(|_| "acme".into());
    let platform = std::env::var("PLATFORM").unwrap_or_else(|_| "webchat".into());

    let queue = bootstrap(&nats_url, &tenant, &platform).await?;
    tracing::info!(
        stream = %queue.stream,
        consumer = %queue.consumer,
        "egress-webchat consuming from JetStream"
    );

    let (tx, _rx) = broadcast::channel::<Value>(64);
    let state = AppState { latest: tx.clone() };
    let translator = WebChatTranslator::new();
    let client = queue.client();
    let mut messages = queue.messages;
    let limiter = queue.limiter;
    let dlq = DlqPublisher::new("egress", client).await?;

    tokio::spawn(async move {
        let dlq = dlq.clone();
        while let Some(next) = messages.next().await {
            let msg = match next {
                Ok(msg) => msg,
                Err(err) => {
                    tracing::error!("jetstream message error: {err}");
                    continue;
                }
            };

            let out: OutMessage = match serde_json::from_slice(msg.payload.as_ref()) {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!("bad out msg: {e}");
                    let _ = msg.ack().await;
                    continue;
                }
            };

            if out.platform != Platform::WebChat {
                if let Err(err) = msg.ack().await {
                    tracing::error!("ack failed: {err}");
                }
                continue;
            }

            let msg_id = out.message_id();
            let span = tracing::info_span!(
                "egress.acquire_permit",
                tenant = %out.tenant,
                platform = "webchat",
                chat_id = %out.chat_id,
                msg_id = %msg_id
            );
            let permit = match limiter.acquire(&out.tenant).instrument(span).await {
                Ok(p) => p,
                Err(err) => {
                    tracing::error!(
                        error = %err,
                        tenant = %out.tenant,
                        platform = "webchat",
                        "failed to acquire backpressure permit"
                    );
                    let _ = msg.ack_with(AckKind::Nak(None)).await;
                    continue;
                }
            };
            event!(
                Level::INFO,
                tenant = %out.tenant,
                platform = "webchat",
                msg_id = %msg_id,
                acquired = true,
                "backpressure permit acquired"
            );

            let translate_span = tracing::info_span!(
                "translate.run",
                tenant = %out.tenant,
                platform = %out.platform.as_str(),
                chat_id = %out.chat_id,
                msg_id = %msg_id
            );
            let result = {
                let _translate_guard = translate_span.enter();
                translator.to_platform(&out)
            }
            .map(|payloads| {
                let send_span = tracing::info_span!(
                    "egress.send",
                    tenant = %out.tenant,
                    platform = %out.platform.as_str(),
                    chat_id = %out.chat_id,
                    msg_id = %msg_id
                );
                let _send_guard = send_span.enter();
                payloads.into_iter().for_each(|payload| {
                    let _ = tx.send(payload);
                });
            });
            drop(permit);

            match result {
                Ok(()) => {
                    if let Err(err) = msg.ack().await {
                        tracing::error!("ack failed: {err}");
                    }
                    metrics::counter!(
                        "messages_egressed",
                        1,
                        "tenant" => out.tenant.clone(),
                        "platform" => out.platform.as_str().to_string()
                    );
                }
                Err(e) => {
                    tracing::warn!("webchat translation failed: {e}");
                    if let Err(err) = dlq
                        .publish(
                            &out.tenant,
                            out.platform.as_str(),
                            &msg_id,
                            1,
                            DlqError {
                                code: "E_TRANSLATE".into(),
                                message: e.to_string(),
                                stage: None,
                            },
                            &out,
                        )
                        .await
                    {
                        tracing::error!("failed to publish dlq entry: {err}");
                    }
                    if let Err(err) = msg.ack_with(AckKind::Nak(None)).await {
                        tracing::error!("nak failed: {err}");
                    }
                }
            }
        }
    });

    let app = Router::new()
        .route("/", get(index))
        .route("/events", get(events))
        .with_state(state);

    let addr: std::net::SocketAddr = std::env::var("BIND")
        .unwrap_or_else(|_| "0.0.0.0:8070".into())
        .parse()
        .unwrap();
    tracing::info!("egress-webchat UI on {}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn index() -> Html<String> {
    let file = ASSETS.get_file("index.html").expect("index.html missing");
    Html(String::from_utf8_lossy(file.contents()).to_string())
}

async fn events(State(state): State<AppState>) -> impl IntoResponse {
    let mut rx = state.latest.subscribe();
    let stream = stream! {
        loop {
            match rx.recv().await {
                Ok(v) => {
                    let line = format!("data: {}\n\n", v);
                    yield Ok::<_, std::io::Error>(axum::body::Bytes::from(line));
                }
                Err(_) => break,
            }
        }
    };
    (
        [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
        axum::body::Body::from_stream(stream),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;
    use http_body_util::BodyExt;
    use serde_json::json;
    use tokio::time::{sleep, Duration};

    #[tokio::test]
    async fn index_returns_html() {
        let Html(body) = index().await;
        assert!(body.contains("<html"), "body should contain html tag");
    }

    #[tokio::test]
    async fn events_sets_event_stream_header() {
        let (tx, _) = broadcast::channel(8);
        let state = AppState { latest: tx };
        let response = events(State(state)).await.into_response();
        assert_eq!(
            response
                .headers()
                .get(axum::http::header::CONTENT_TYPE)
                .unwrap(),
            "text/event-stream"
        );
    }

    #[tokio::test]
    async fn events_streams_payload() {
        let (tx, _) = broadcast::channel(8);
        let state = AppState { latest: tx.clone() };

        let response = events(State(state)).await.into_response();
        let body = response.into_body();

        tokio::spawn(async move {
            sleep(Duration::from_millis(10)).await;
            let _ = tx.send(json!({ "kind": "text", "text": "hello" }));
        });

        let collected = body.collect().await.expect("collect body");
        let content = String::from_utf8(collected.to_bytes().to_vec()).expect("utf8");
        assert!(
            content.contains("data: {\"kind\":\"text\",\"text\":\"hello\"}"),
            "stream should contain serialized payload"
        );
    }
}
