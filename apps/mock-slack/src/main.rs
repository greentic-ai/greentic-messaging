use anyhow::Result;
use axum::{Json, Router, routing::post};
use greentic_telemetry::{TelemetryConfig, init_telemetry};
use serde_json::Value;
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> Result<()> {
    init_telemetry(TelemetryConfig {
        service_name: "greentic-messaging".into(),
    })?;
    let app = Router::new().route("/events", post(handle));
    let listener = TcpListener::bind("0.0.0.0:9082").await?;
    tracing::info!("mock-slack listening on {}", listener.local_addr()?);
    axum::serve(listener, app).await?;
    Ok(())
}

async fn handle(Json(payload): Json<Value>) -> &'static str {
    tracing::info!("SLACK EVENTS: {}", payload);
    "ok"
}
