use anyhow::{Error, Result};
use async_nats::Client;
use futures::StreamExt;
use gsm_core::*;
use gsm_telemetry::install as init_telemetry;

#[tokio::main]
async fn main() -> Result<()> {
    init_telemetry("greentic-messaging")?;
    let nats_url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
    let env = std::env::var("GREENTIC_ENV").unwrap_or_else(|_| "dev".into());
    let tenant = std::env::var("TENANT").unwrap_or_else(|_| "acme".into());
    let team = std::env::var("TEAM").unwrap_or_else(|_| "default".into());
    let platform = Platform::Telegram;
    let chat_id = "demo-chat-1";

    let client = async_nats::connect(nats_url).await?;
    subscribe_out(&client, &env, &tenant, &team, &platform).await?;
    publish_in(&client, &env, &tenant, &team, &platform, chat_id).await?;
    Ok(())
}

async fn subscribe_out(
    client: &Client,
    env: &str,
    tenant: &str,
    team: &str,
    platform: &Platform,
) -> Result<()> {
    let subject = egress_subject(env, tenant, team, platform.as_str());
    let mut sub = client.subscribe(subject.clone()).await?;
    tokio::spawn(async move {
        while let Some(msg) = sub.next().await {
            println!(
                "OUT [{}]: {}",
                subject,
                String::from_utf8_lossy(&msg.payload)
            );
        }
    });
    Ok(())
}

async fn publish_in(
    client: &Client,
    env: &str,
    tenant: &str,
    team: &str,
    platform: &Platform,
    chat_id: &str,
) -> Result<()> {
    let envelope = MessageEnvelope {
        tenant: tenant.to_string(),
        platform: platform.clone(),
        chat_id: chat_id.to_string(),
        user_id: "u1".into(),
        thread_id: None,
        msg_id: "m1".into(),
        text: Some("hello world".into()),
        timestamp: "2025-10-14T09:00:00Z".into(),
        context: Default::default(),
    };
    let subject = ingress_subject(env, tenant, team, platform.as_str());
    let invocation = envelope.into_invocation().map_err(Error::new)?;
    let bytes = serde_json::to_vec(&invocation)?;
    client.publish(subject, bytes.into()).await?;
    Ok(())
}
