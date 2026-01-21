use anyhow::{Error, Result};
use async_nats::jetstream::{
    consumer::AckPolicy, consumer::push::Config as PushConfig, stream::Config as StreamConfig,
    stream::RetentionPolicy,
};
use futures::StreamExt;
use gsm_core::{
    AdapterDescriptor, AdapterRegistry, DefaultAdapterPacksConfig, HttpRunnerClient,
    InMemoryProviderInstallStore, LoggingRunnerClient, OutMessage, ProviderInstallError,
    ProviderInstallState, ProviderInstallStore, RunnerClient, apply_install_refs,
    default_adapter_pack_paths, extract_provider_route, load_install_store_from_path,
    shared_client,
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
    let install_store = if let Some(path) = config.install_store_path.as_ref() {
        match load_install_store_from_path(path.as_path()) {
            Ok(store) => Arc::new(store),
            Err(err) => {
                warn!(error = %err, path = %path.display(), "failed to load install records");
                Arc::new(InMemoryProviderInstallStore::default())
            }
        }
    } else {
        Arc::new(InMemoryProviderInstallStore::default())
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
                    install_store.as_ref(),
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
    install_store: &dyn ProviderInstallStore,
) -> Result<(), Error> {
    let out: OutMessage = serde_json::from_slice(&msg.payload)?;
    info!(
        env = %out.ctx.env.as_str(),
        tenant = %out.tenant,
        platform = %out.platform.as_str(),
        chat_id = %out.chat_id,
        "received OutMessage for processing"
    );
    let install_state = resolve_install_for_egress(install_store, &out)?;
    let adapter = if let Some(name) = adapter_override {
        adapters.egress(name)?
    } else {
        adapters.default_for_platform(out.platform.as_str())?
    };
    validate_adapter_for_install(&install_state, &adapter)?;
    info!(
        adapter = %adapter.name,
        component = %adapter.component,
        flow = ?adapter.flow_path(),
        tenant = %out.tenant,
        platform = %out.platform.as_str(),
        "resolved egress adapter"
    );
    process_message_internal(&out, &adapter, bus, runner, config, &install_state).await
}

/// Internal helper used by tests to avoid NATS.
pub async fn process_message_internal(
    out: &OutMessage,
    adapter: &AdapterDescriptor,
    bus: &impl BusClient,
    runner: &dyn RunnerClient,
    config: &EgressConfig,
    install_state: &ProviderInstallState,
) -> Result<(), Error> {
    let mut routed = out.clone();
    apply_install_refs(&mut routed.meta, &install_state.record);
    if let Err(err) = runner.invoke_adapter(&routed, adapter).await {
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
        "tenant" => routed.tenant.clone(),
        "platform" => out.platform.as_str().to_string(),
        "adapter" => adapter.name.clone()
    );

    let team = routed
        .ctx
        .team
        .as_ref()
        .map(|team| team.as_str())
        .unwrap_or("default");
    let subject = gsm_core::egress_subject_with_prefix(
        config.egress_prefix.as_str(),
        routed.ctx.env.as_str(),
        routed.ctx.tenant.as_str(),
        team,
        routed.platform.as_str(),
    );
    let payload = serde_json::json!({
        "tenant": routed.tenant,
        "platform": routed.platform.as_str(),
        "chat_id": routed.chat_id,
        "text": routed.text,
        "kind": routed.kind,
        "metadata": routed.meta,
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
        "tenant" => routed.tenant.clone(),
        "platform" => routed.platform.as_str().to_string(),
        "adapter" => adapter.name.clone()
    );
    Ok(())
}

fn resolve_install_for_egress(
    store: &dyn ProviderInstallStore,
    out: &OutMessage,
) -> Result<ProviderInstallState, ProviderInstallError> {
    let (provider_id, install_id) =
        extract_provider_route(&out.meta).ok_or(ProviderInstallError::MissingRoute)?;
    let state = store
        .get(&out.ctx, &provider_id, &install_id)
        .ok_or_else(|| ProviderInstallError::MissingInstall {
            provider_id,
            install_id: install_id.to_string(),
        })?;
    enforce_install_state(&state)?;
    Ok(state)
}

fn enforce_install_state(state: &ProviderInstallState) -> Result<(), ProviderInstallError> {
    for key in state.record.secret_refs.keys() {
        if !state.secrets.contains_key(key) {
            return Err(ProviderInstallError::MissingSecret { key: key.clone() });
        }
    }
    for key in state.record.config_refs.keys() {
        if !state.config.contains_key(key) {
            return Err(ProviderInstallError::MissingConfig { key: key.clone() });
        }
    }
    Ok(())
}

fn validate_adapter_for_install(
    install_state: &ProviderInstallState,
    adapter: &AdapterDescriptor,
) -> Result<(), Error> {
    let expected = install_state.record.pack_id.as_str();
    if adapter.pack_id != expected {
        anyhow::bail!(
            "adapter {} does not match install pack {}",
            adapter.pack_id,
            expected
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use greentic_types::{
        EnvId, PackId, ProviderInstallId, ProviderInstallRecord, TenantCtx, TenantId,
    };
    use gsm_core::{ProviderInstallState, make_tenant_ctx};
    use semver::Version;
    use serde_json::Value;
    use std::collections::BTreeMap;
    use time::OffsetDateTime;

    fn install_record(install_id: &str) -> ProviderInstallState {
        let tenant = TenantCtx::new(
            "dev".parse::<EnvId>().expect("env"),
            "acme".parse::<TenantId>().expect("tenant"),
        );
        let mut config_refs = BTreeMap::new();
        config_refs.insert("config".to_string(), "state:config".to_string());
        let mut secret_refs = BTreeMap::new();
        secret_refs.insert("token".to_string(), "secrets:token".to_string());
        let record = ProviderInstallRecord {
            tenant,
            provider_id: "messaging.slack".to_string(),
            install_id: install_id.parse::<ProviderInstallId>().expect("install id"),
            pack_id: "messaging-slack".parse::<PackId>().expect("pack id"),
            pack_version: Version::parse("1.0.0").expect("version"),
            created_at: OffsetDateTime::from_unix_timestamp(1_700_000_000).expect("created_at"),
            updated_at: OffsetDateTime::from_unix_timestamp(1_700_000_100).expect("updated_at"),
            config_refs,
            secret_refs,
            webhook_state: serde_json::json!({}),
            subscriptions_state: serde_json::json!({}),
            metadata: serde_json::json!({}),
        };
        let mut state = ProviderInstallState::new(record);
        state.secrets.insert("token".into(), "secret".into());
        state
            .config
            .insert("config".into(), serde_json::json!({"ok": true}));
        state
    }

    #[test]
    fn resolves_install_from_outbound_meta() {
        let store = InMemoryProviderInstallStore::default();
        let install_state = install_record("install-a");
        store.insert(install_state.clone());

        let ctx = make_tenant_ctx("acme".into(), None, None);
        let mut meta = BTreeMap::new();
        apply_install_refs(&mut meta, &install_state.record);
        let out = OutMessage {
            ctx,
            tenant: "acme".into(),
            platform: gsm_core::Platform::Slack,
            chat_id: "chat-1".into(),
            thread_id: None,
            kind: gsm_core::OutKind::Text,
            text: Some("hi".into()),
            message_card: None,
            adaptive_card: None,
            meta,
        };

        let resolved = resolve_install_for_egress(&store, &out).expect("resolve install");
        assert_eq!(resolved.record.install_id.as_str(), "install-a");
    }

    #[test]
    fn returns_error_when_install_missing() {
        let store = InMemoryProviderInstallStore::default();
        let ctx = make_tenant_ctx("acme".into(), None, None);
        let mut meta = BTreeMap::new();
        meta.insert(
            "provider_id".into(),
            Value::String("messaging.slack".into()),
        );
        meta.insert("install_id".into(), Value::String("missing".into()));
        let out = OutMessage {
            ctx,
            tenant: "acme".into(),
            platform: gsm_core::Platform::Slack,
            chat_id: "chat-1".into(),
            thread_id: None,
            kind: gsm_core::OutKind::Text,
            text: Some("hi".into()),
            message_card: None,
            adaptive_card: None,
            meta,
        };

        let err = resolve_install_for_egress(&store, &out).unwrap_err();
        assert!(matches!(err, ProviderInstallError::MissingInstall { .. }));
    }

    #[test]
    fn returns_error_when_secret_missing() {
        let store = InMemoryProviderInstallStore::default();
        let mut state = install_record("install-a");
        state.secrets.clear();
        store.insert(state.clone());

        let ctx = make_tenant_ctx("acme".into(), None, None);
        let mut meta = BTreeMap::new();
        apply_install_refs(&mut meta, &state.record);
        let out = OutMessage {
            ctx,
            tenant: "acme".into(),
            platform: gsm_core::Platform::Slack,
            chat_id: "chat-1".into(),
            thread_id: None,
            kind: gsm_core::OutKind::Text,
            text: Some("hi".into()),
            message_card: None,
            adaptive_card: None,
            meta,
        };

        let err = resolve_install_for_egress(&store, &out).unwrap_err();
        assert!(matches!(err, ProviderInstallError::MissingSecret { .. }));
    }

    #[test]
    fn returns_error_when_config_missing() {
        let store = InMemoryProviderInstallStore::default();
        let mut state = install_record("install-a");
        state.config.clear();
        store.insert(state.clone());

        let ctx = make_tenant_ctx("acme".into(), None, None);
        let mut meta = BTreeMap::new();
        apply_install_refs(&mut meta, &state.record);
        let out = OutMessage {
            ctx,
            tenant: "acme".into(),
            platform: gsm_core::Platform::Slack,
            chat_id: "chat-1".into(),
            thread_id: None,
            kind: gsm_core::OutKind::Text,
            text: Some("hi".into()),
            message_card: None,
            adaptive_card: None,
            meta,
        };

        let err = resolve_install_for_egress(&store, &out).unwrap_err();
        assert!(matches!(err, ProviderInstallError::MissingConfig { .. }));
    }
}
