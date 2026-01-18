use anyhow::{Error, Result};
use async_nats::jetstream::{
    consumer::AckPolicy, consumer::push::Config as PushConfig, stream::Config as StreamConfig,
    stream::RetentionPolicy,
};
use futures::StreamExt;
use gsm_core::{
    AdapterDescriptor, AdapterRegistry, DefaultAdapterPacksConfig, HttpRunnerClient,
    LoggingRunnerClient, OutMessage, RunnerClient, default_adapter_pack_paths, shared_client,
};
use metrics::counter;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{error, info, warn};

use crate::adapter_registry::AdapterLookup;
use crate::config::EgressConfig;
use gsm_bus::{BusClient, NatsBusClient, to_value};

pub async fn run() -> Result<()> {
    let config = EgressConfig::load()?;
    gsm_core::set_current_env(config.env.clone());
    let client = async_nats::connect(&config.nats_url).await?;
    let js = async_nats::jetstream::new(client.clone());

    let default_cfg = DefaultAdapterPacksConfig::default();
    let mut pack_paths =
        default_adapter_pack_paths(PathBuf::from(&config.packs_root).as_path(), &default_cfg);
    let extra_paths: Vec<PathBuf> = Vec::new();
    pack_paths.extend(extra_paths);
    let packs_root = PathBuf::from(&config.packs_root);
    let registry = AdapterRegistry::load_from_paths(packs_root.as_path(), &pack_paths)
        .unwrap_or_else(|err| {
            warn!(error = %err, "failed to load adapter packs; proceeding without registry");
            AdapterRegistry::default()
        });
    let adapters = AdapterLookup::new(&registry);
    let bus = NatsBusClient::new(client.clone());
    let runner_client: Arc<dyn RunnerClient> = match &config.runner_http_url {
        Some(url) => shared_client(HttpRunnerClient::new(
            url.clone(),
            config.runner_http_api_key.clone(),
        )?),
        None => shared_client(LoggingRunnerClient),
    };

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
                if let Err(err) = process_message(
                    &message,
                    &adapters,
                    config.adapter.as_deref(),
                    &bus,
                    &*runner_client,
                    &config,
                )
                .await
                {
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

async fn process_message(
    msg: &async_nats::jetstream::Message,
    adapters: &AdapterLookup<'_>,
    adapter_override: Option<&str>,
    bus: &impl BusClient,
    runner: &dyn RunnerClient,
    config: &EgressConfig,
) -> Result<(), Error> {
    let out: OutMessage = serde_json::from_slice(&msg.payload)?;
    info!(
        env = %out.ctx.env.as_str(),
        tenant = %out.tenant,
        platform = %out.platform.as_str(),
        chat_id = %out.chat_id,
        "received OutMessage for processing"
    );
    let adapter = if let Some(name) = adapter_override {
        adapters.egress(name)?
    } else {
        adapters.default_for_platform(out.platform.as_str())?
    };
    info!(
        adapter = %adapter.name,
        component = %adapter.component,
        flow = ?adapter.flow_path(),
        tenant = %out.tenant,
        platform = %out.platform.as_str(),
        "resolved egress adapter"
    );
    process_message_internal(&out, &adapter, bus, runner, config).await
}

/// Internal helper used by tests to avoid NATS.
pub async fn process_message_internal(
    out: &OutMessage,
    adapter: &AdapterDescriptor,
    bus: &impl BusClient,
    runner: &dyn RunnerClient,
    config: &EgressConfig,
) -> Result<(), Error> {
    if let Err(err) = runner.invoke_adapter(out, adapter).await {
        let _ = counter!(
            "messaging_egress_runner_failure_total",
            "tenant" => out.tenant.clone(),
            "platform" => out.platform.as_str().to_string(),
            "adapter" => adapter.name.clone()
        );
        tracing::error!(
            tenant = %out.tenant,
            platform = %out.platform.as_str(),
            adapter = %adapter.name,
            error = %err,
            "runner invocation failed"
        );
        return Err(err);
    }
    let _ = counter!(
        "messaging_egress_runner_success_total",
        "tenant" => out.tenant.clone(),
        "platform" => out.platform.as_str().to_string(),
        "adapter" => adapter.name.clone()
    );

    let team = out
        .ctx
        .team
        .as_ref()
        .map(|team| team.as_str())
        .unwrap_or("default");
    let subject = gsm_core::egress_subject_with_prefix(
        config.egress_prefix.as_str(),
        out.ctx.env.as_str(),
        out.ctx.tenant.as_str(),
        team,
        out.platform.as_str(),
    );
    let payload = serde_json::json!({
        "tenant": out.tenant,
        "platform": out.platform.as_str(),
        "chat_id": out.chat_id,
        "text": out.text,
        "kind": out.kind,
        "metadata": out.meta,
        "adapter": adapter.name,
    });
    let value = to_value(&payload)?;
    bus.publish_value(&subject, value).await.map_err(|err| {
        tracing::error!(
            %subject,
            tenant = %out.tenant,
            platform = %out.platform.as_str(),
            error = %err,
            "failed to publish egress envelope"
        );
        anyhow::Error::new(err)
    })?;
    let _ = counter!(
        "messaging_egress_total",
        "tenant" => out.tenant.clone(),
        "platform" => out.platform.as_str().to_string(),
        "adapter" => adapter.name.clone()
    );
    Ok(())
}
