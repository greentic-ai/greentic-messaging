mod config;

use anyhow::{Error, Result};
use async_nats::jetstream::{
    consumer::AckPolicy, consumer::push::Config as PushConfig, stream::Config as StreamConfig,
    stream::RetentionPolicy,
};
use futures::StreamExt;
use gsm_core::OutMessage;
use tracing::{error, info, warn};

use crate::config::EgressConfig;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let config = EgressConfig::from_env()?;
    let client = async_nats::connect(&config.nats_url).await?;
    let js = async_nats::jetstream::new(client.clone());

    let stream_name = format!("messaging-egress-{}", config.env.0);
    let stream = js
        .get_or_create_stream(StreamConfig {
            name: stream_name.clone(),
            subjects: vec![config.subject_filter.clone()],
            retention: RetentionPolicy::WorkQueue,
            max_messages: -1,
            max_messages_per_subject: -1,
            max_bytes: -1,
            ..Default::default()
        })
        .await?;

    let consumer_name = format!("messaging-egress-{}", config.env.0);
    let deliver_subject = format!("deliver.messaging-egress.{}", config.env.0);
    let consumer = stream
        .get_or_create_consumer(
            &consumer_name,
            PushConfig {
                durable_name: Some(consumer_name.clone()),
                deliver_subject: deliver_subject.clone(),
                deliver_group: Some(format!("messaging-egress-{}", config.env.0)),
                ack_policy: AckPolicy::Explicit,
                max_ack_pending: 128,
                ..Default::default()
            },
        )
        .await?;

    info!(
        subject = %config.subject_filter,
        stream = %stream_name,
        consumer = %consumer_name,
        "messaging-egress listening for envelopes"
    );

    let mut messages = consumer.messages().await?;

    while let Some(result) = messages.next().await {
        match result {
            Ok(message) => {
                if let Err(err) = process_message(&message).await {
                    error!(error = %err, "failed to process egress payload");
                }
                if let Err(err) = message.ack().await {
                    warn!(error = %err, "failed to ack egress delivery");
                }
            }
            Err(err) => {
                warn!(error = %err, "missing message from JetStream");
            }
        }
    }

    Ok(())
}

async fn process_message(msg: &async_nats::jetstream::Message) -> Result<(), Error> {
    let out: OutMessage = serde_json::from_slice(&msg.payload)?;
    info!(
        env = %out.ctx.env.as_str(),
        tenant = %out.tenant,
        platform = %out.platform.as_str(),
        chat_id = %out.chat_id,
        "received OutMessage for processing"
    );
    Ok(())
}
