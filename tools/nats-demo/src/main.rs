use anyhow::Result;
use async_nats::Client;
use futures::StreamExt;
use gsm_core::*;

#[tokio::main]
async fn main() -> Result<()> {
    let nats_url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
    let tenant = std::env::var("TENANT").unwrap_or_else(|_| "acme".into());
    let platform = Platform::Telegram;
    let chat_id = "demo-chat-1";

    let client = async_nats::connect(nats_url).await?;
    subscribe_out(&client, &tenant, &platform, chat_id).await?;
    publish_in(&client, &tenant, &platform, chat_id).await?;
    Ok(())
}

async fn subscribe_out(
    client: &Client,
    tenant: &str,
    platform: &Platform,
    chat_id: &str,
) -> Result<()> {
    let subject = out_subject(tenant, platform.as_str(), chat_id);
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
    tenant: &str,
    platform: &Platform,
    chat_id: &str,
) -> Result<()> {
    let env = MessageEnvelope {
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
    let subject = in_subject(tenant, platform.as_str(), chat_id);
    let bytes = serde_json::to_vec(&env)?;
    client.publish(subject, bytes.into()).await?;
    Ok(())
}
