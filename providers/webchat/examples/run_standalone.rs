use greentic_messaging_providers_webchat::config::Config;
use greentic_messaging_providers_webchat::{StandaloneState, standalone_router};
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::from_env();
    let state = Arc::new(StandaloneState::new(config).await?);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8090").await?;
    let app = standalone_router(Arc::clone(&state));
    axum::serve(listener, app.into_make_service()).await?;
    Ok(())
}
