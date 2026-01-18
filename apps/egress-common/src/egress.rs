use std::sync::Arc;

use anyhow::{Context, Result};
use async_nats::{
    Client,
    jetstream::{
        consumer::{
            AckPolicy,
            push::{Config as PushConfig, Messages},
        },
        stream::{Config as StreamConfig, RetentionPolicy},
    },
};
use gsm_backpressure::HybridLimiter;

/// Shared bootstrap for egress workers: connects to NATS, ensures the JetStream
/// queue-group consumer exists, and returns a ready-to-use message stream plus
/// the distributed rate limiter.
pub struct QueueConsumer {
    // Keep the client alive so the JetStream context backing the message stream
    // remains valid for the life of the worker.
    #[allow(dead_code)]
    client: Client,
    pub messages: Messages,
    pub limiter: Arc<HybridLimiter>,
    pub stream: String,
    pub consumer: String,
}

impl QueueConsumer {
    pub fn client(&self) -> Client {
        self.client.clone()
    }
}

/// Connect to NATS JetStream and prepare a queue-group consumer for the egress worker.
pub async fn bootstrap(nats_url: &str, env: &str, platform: &str) -> Result<QueueConsumer> {
    let client = async_nats::connect(nats_url).await?;
    let js = async_nats::jetstream::new(client.clone());
    let limiter = HybridLimiter::new(Some(&js)).await?;

    // Subscribe to all tenants for this platform; tenant context is carried in the message.
    let subject = format!(
        "{}.{}.*.*.{}",
        gsm_core::EGRESS_SUBJECT_PREFIX,
        env,
        platform
    );
    let stream_name = format!("msg-out-{}-{platform}", env);
    let stream_cfg = StreamConfig {
        name: stream_name.clone(),
        subjects: vec![subject.clone()],
        retention: RetentionPolicy::WorkQueue,
        max_messages: -1,
        max_messages_per_subject: -1,
        max_bytes: -1,
        ..Default::default()
    };

    let stream = js
        .get_or_create_stream(stream_cfg)
        .await
        .with_context(|| format!("ensure stream {stream_name}"))?;

    let deliver_subject = format!("deliver.egress.{}.{}", env, platform);
    let consumer_name = format!("egress-{}-{platform}", env);
    let consumer = stream
        .get_or_create_consumer(
            &consumer_name,
            PushConfig {
                durable_name: Some(consumer_name.clone()),
                deliver_subject,
                deliver_group: Some(format!("egress-{platform}")),
                filter_subject: subject,
                ack_policy: AckPolicy::Explicit,
                max_ack_pending: 256,
                ..Default::default()
            },
        )
        .await
        .with_context(|| format!("ensure consumer {consumer_name}"))?;

    let messages = consumer
        .messages()
        .await
        .with_context(|| format!("attach consumer stream {consumer_name}"))?;

    Ok(QueueConsumer {
        client,
        messages,
        limiter,
        stream: stream_name,
        consumer: consumer_name,
    })
}
