//! WebChat egress adapter that streams outbound `OutMessage`s to a simple SSE UI.
//!
//! ```text
//! Start the binary, then open `/events` to watch translated payloads
//! as they arrive from NATS.
//! ```

use anyhow::Result;
use async_stream::stream;
use axum::{
    extract::State,
    response::{Html, IntoResponse},
    routing::get,
    Router,
};
use futures::StreamExt;
use gsm_core::{OutMessage, Platform};
use gsm_translator::{Translator, WebChatTranslator};
use include_dir::{include_dir, Dir};
use serde_json::Value;
use tokio::sync::broadcast;
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

    let nats = async_nats::connect(nats_url).await?;
    let subject = format!("greentic.msg.out.{}.{}.>", tenant, platform);
    let mut sub = nats.subscribe(subject.clone()).await?;
    tracing::info!("egress-webchat subscribed to {subject}");

    let (tx, _rx) = broadcast::channel::<Value>(64);
    let state = AppState { latest: tx.clone() };
    let translator = WebChatTranslator::new();

    tokio::spawn(async move {
        while let Some(msg) = sub.next().await {
            match serde_json::from_slice::<OutMessage>(&msg.payload) {
                Ok(out) if out.platform == Platform::WebChat => {
                    if let Ok(payloads) = translator.to_platform(&out) {
                        for payload in payloads {
                            let _ = tx.send(payload);
                        }
                    }
                }
                Ok(_) => {}
                Err(e) => tracing::warn!("bad out msg: {e}"),
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
