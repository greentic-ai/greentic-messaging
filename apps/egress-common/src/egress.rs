use std::sync::Arc;

use anyhow::{Context, Result};
use async_nats::{
    jetstream::{
        consumer::{
            push::{Config as PushConfig, Messages},
            AckPolicy,
        },
        stream::{Config as StreamConfig, RetentionPolicy},
    },
    Client,
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
///
/// ```no_run
/// use gsm_egress_common::bootstrap;
///
/// # fn main() -> anyhow::Result<()> {
/// # let rt = tokio::runtime::Runtime::new()?;
/// rt.block_on(async {
///     let consumer = bootstrap("nats://127.0.0.1:4222", "acme", "webex").await?;
///     println!("stream={} consumer={}", consumer.stream, consumer.consumer);
///     anyhow::Ok(())
/// })
/// # }
/// ```
pub async fn bootstrap(nats_url: &str, tenant: &str, platform: &str) -> Result<QueueConsumer> {
    let client = async_nats::connect(nats_url).await?;
    let js = async_nats::jetstream::new(client.clone());
    let limiter = HybridLimiter::new(Some(&js)).await?;

    let subject = format!("greentic.msg.out.{}.{}.>", tenant, platform);
    let stream_name = format!("msg-out-{}-{}", tenant, platform);
    let mut stream_cfg = StreamConfig::default();
    stream_cfg.name = stream_name.clone();
    stream_cfg.subjects = vec![subject.clone()];
    stream_cfg.retention = RetentionPolicy::WorkQueue;
    stream_cfg.max_messages = -1;
    stream_cfg.max_messages_per_subject = -1;
    stream_cfg.max_bytes = -1;

    let stream = js
        .get_or_create_stream(stream_cfg)
        .await
        .with_context(|| format!("ensure stream {stream_name}"))?;

    let deliver_subject = format!("deliver.egress.{tenant}.{platform}");
    let consumer_name = format!("egress-{tenant}-{platform}");
    let consumer = stream
        .get_or_create_consumer(
            &consumer_name,
            PushConfig {
                durable_name: Some(consumer_name.clone()),
                deliver_subject,
                deliver_group: Some(format!("egress-{tenant}")),
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
