use anyhow::Result;
use axum::{Json, Router, routing::post};
use chrono::Utc;
use gsm_telemetry::install as init_telemetry;
use serde::Deserialize;
use serde_json::{Value as JsonValue, json};

#[derive(Debug, Deserialize)]
struct ForecastRequest {
    q: Option<String>,
    days: Option<JsonValue>,
}

#[tokio::main]
async fn main() -> Result<()> {
    init_telemetry("greentic-messaging")?;
    let app = Router::new().route("/weather_api/forecast_weather", post(handle_forecast));

    let addr: std::net::SocketAddr = std::env::var("BIND")
        .unwrap_or_else(|_| "0.0.0.0:18081".into())
        .parse()
        .unwrap();
    tracing::info!("mock weather tool listening on {}", addr);
    axum::serve(tokio::net::TcpListener::bind(addr).await?, app).await?;
    Ok(())
}

async fn handle_forecast(Json(req): Json<ForecastRequest>) -> Json<serde_json::Value> {
    let q = req.q.unwrap_or_else(|| "Unknown".to_string());
    let days = parse_days(req.days).unwrap_or(1).clamp(1, 7);

    let today = Utc::now().date_naive();
    let mut forecastday = Vec::new();
    for i in 0..days {
        let date = today + chrono::Duration::days(i as i64);
        forecastday.push(json!({
            "date": date.to_string(),
            "day": {
                "maxtemp_c": 20 - i as i32,
                "mintemp_c": 12 - i as i32,
                "condition": { "text": "Partly cloudy" },
                "daily_will_it_rain": 0
            }
        }));
    }

    let body = json!({
        "location": { "name": q, "lat": 51.5, "lon": -0.1 },
        "forecast": { "forecastday": forecastday }
    });

    tracing::info!("responding with forecast");
    Json(body)
}

fn parse_days(value: Option<JsonValue>) -> Option<u32> {
    match value {
        Some(JsonValue::Number(n)) => n.as_u64().map(|v| v as u32),
        Some(JsonValue::String(s)) => s.parse::<u32>().ok(),
        Some(other) => {
            tracing::warn!(?other, "unexpected days value, defaulting to 1");
            None
        }
        None => None,
    }
}
