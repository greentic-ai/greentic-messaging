use anyhow::Result;
use axum::{Json, Router, routing::post};
use gsm_telemetry::install as init_telemetry;
use serde_json::Value;
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> Result<()> {
    init_telemetry("greentic-messaging")?;
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
