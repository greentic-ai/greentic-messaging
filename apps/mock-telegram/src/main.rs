use anyhow::Result;
use axum::{routing::post, Json, Router};
use serde_json::Value;
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();
    let app = Router::new().route("/webhook", post(handle));
    let listener = TcpListener::bind("0.0.0.0:9081").await?;
    tracing::info!("mock-telegram listening on {}", listener.local_addr()?);
    axum::serve(listener, app).await?;
    Ok(())
}

async fn handle(Json(payload): Json<Value>) -> &'static str {
    tracing::info!("TELEGRAM WEBHOOK: {}", payload);
    "ok"
}
