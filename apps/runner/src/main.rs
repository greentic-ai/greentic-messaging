mod card_node;
mod flow_registry;
mod model;
mod qa_node;
mod template_node;
mod tool_node;

use anyhow::Result;
use async_nats::Client as Nats;
use futures::StreamExt;
use greentic_config::ConfigResolver;
use greentic_config_types::{GreenticConfig, ServiceTransportConfig};
use greentic_types::{FlowId, PackId, SessionCursor, SessionKey, UserId};
use gsm_core::*;
use gsm_dlq::{DlqConfig, DlqError, DlqPublisher, replay_subject_with_config};
use gsm_session::{SessionData, SharedSessionStore, store_from_env};
use gsm_telemetry::{
    AuthRenderMode, MessageContext, TelemetryLabels, install as init_telemetry,
    record_auth_card_render, set_current_tenant_ctx,
};
use serde_json::json;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use crate::flow_registry::FlowRegistry;

struct RunnerConfig {
    env: EnvId,
    nats_url: String,
    packs_root: PathBuf,
    default_packs: DefaultAdapterPacksConfig,
    extra_pack_paths: Vec<PathBuf>,
    tool_endpoint: String,
    dlq: DlqConfig,
}

impl RunnerConfig {
    fn load() -> Result<Self> {
        let resolved = ConfigResolver::new().load()?;
        Self::from_config(&resolved.config)
    }

    fn from_config(config: &GreenticConfig) -> Result<Self> {
        let env = config.environment.env_id.clone();
        let nats_url =
            nats_url_from_config(config)?.unwrap_or_else(|| "nats://127.0.0.1:4222".to_string());
        let packs_root = config.paths.greentic_root.join("packs");
        let tool_endpoint =
            tool_endpoint_from_config(config).unwrap_or_else(|| "http://localhost:18081".into());
        Ok(Self {
            env,
            nats_url,
            packs_root,
            default_packs: DefaultAdapterPacksConfig::default(),
            extra_pack_paths: Vec::new(),
            tool_endpoint,
            dlq: DlqConfig::default(),
        })
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    init_telemetry("greentic-messaging")?;
    let config = RunnerConfig::load()?;
    set_current_env(config.env.clone());
    if !config.packs_root.exists() {
        let _ = std::fs::create_dir_all(&config.packs_root);
    }
    let mut pack_paths =
        default_adapter_pack_paths(config.packs_root.as_path(), &config.default_packs);
    pack_paths.extend(config.extra_pack_paths.clone());

    let flow_registry = FlowRegistry::load_from_paths(config.packs_root.as_path(), &pack_paths)?;
    if flow_registry.is_empty() {
        return Err(anyhow::anyhow!("no flows loaded from pack metadata"));
    }

    let nats = async_nats::connect(config.nats_url).await?;
    let dlq = DlqPublisher::new_with_config("translate", nats.clone(), config.dlq.clone()).await?;

    let subject = format!(
        "{}.{}.>",
        gsm_core::INGRESS_SUBJECT_PREFIX,
        config.env.as_str()
    );
    let mut sub = nats.subscribe(subject.clone()).await?;
    tracing::info!("runner subscribed to {subject}");

    let replay_subject = replay_subject_with_config(&config.dlq, "*", "translate");
    let mut replay_sub = nats.subscribe(replay_subject.clone()).await?;
    tracing::info!("runner subscribed to {replay_subject} for replays");

    let hbs = template_node::hb_registry();
    let sessions = store_from_env().await?;

    let ctx = Arc::new(ProcessContext {
        nats: nats.clone(),
        flow_registry: Arc::new(flow_registry),
        hbs: hbs.clone(),
        sessions: sessions.clone(),
        dlq: dlq.clone(),
        tool_endpoint: config.tool_endpoint.clone(),
    });

    {
        let ctx = Arc::clone(&ctx);
        tokio::spawn(async move {
            while let Some(msg) = replay_sub.next().await {
                match serde_json::from_slice::<ChannelMessage>(&msg.payload) {
                    Ok(env) => handle_env(ctx.clone(), env).await,
                    Err(e) => tracing::warn!("bad replay envelope: {e}"),
                }
            }
        });
    }

    while let Some(msg) = sub.next().await {
        let channel: ChannelMessage = match serde_json::from_slice(&msg.payload) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("bad channel envelope: {e}");
                continue;
            }
        };
        let ctx = Arc::clone(&ctx);
        tokio::spawn(async move {
            handle_env(ctx, channel).await;
        });
    }

    Ok(())
}

fn nats_url_from_config(config: &GreenticConfig) -> Result<Option<String>> {
    let transport = config
        .services
        .as_ref()
        .and_then(|services| services.events_transport.as_ref())
        .and_then(|svc| svc.transport.as_ref());
    match transport {
        Some(ServiceTransportConfig::Nats { url, .. }) => Ok(Some(url.to_string())),
        Some(ServiceTransportConfig::Http { .. }) => {
            anyhow::bail!("services.events_transport must use NATS for runner");
        }
        Some(ServiceTransportConfig::Noop) => Ok(None),
        None => Ok(None),
    }
}

fn tool_endpoint_from_config(config: &GreenticConfig) -> Option<String> {
    let transport = config
        .services
        .as_ref()
        .and_then(|services| services.metadata.as_ref())
        .and_then(|svc| svc.transport.as_ref());
    match transport {
        Some(ServiceTransportConfig::Http { url, .. }) => Some(url.to_string()),
        _ => None,
    }
}

struct RunOneContext {
    nats: Nats,
    flow: model::Flow,
    hbs: handlebars::Handlebars<'static>,
    sessions: SharedSessionStore,
    tenant_ctx: TenantCtx,
    env: MessageEnvelope,
    tool_endpoint: String,
    pack_id: Option<PackId>,
}

async fn run_one(ctx: RunOneContext) -> Result<()> {
    let active_user = ctx
        .tenant_ctx
        .user
        .clone()
        .or_else(|| ctx.tenant_ctx.user_id.clone())
        .or_else(|| UserId::try_from(ctx.env.user_id.as_str()).ok());
    let mut previous_session: Option<SessionKey> = None;
    let mut state = if let Some(user) = active_user.clone() {
        match ctx.sessions.find_by_user(&ctx.tenant_ctx, &user).await {
            Ok(Some((key, data))) => {
                previous_session = Some(key);
                match serde_json::from_str::<serde_json::Value>(&data.context_json) {
                    Ok(value) if value.is_object() => value,
                    Ok(_) => json!({}),
                    Err(err) => {
                        tracing::warn!(error = %err, "failed to parse stored session context");
                        json!({})
                    }
                }
            }
            Ok(None) => json!({}),
            Err(err) => {
                tracing::warn!(error = %err, "session lookup failed; starting fresh");
                json!({})
            }
        }
    } else {
        json!({})
    };

    let mut current = ctx.flow.r#in.clone();
    let mut payload: serde_json::Value = serde_json::json!({});

    loop {
        let node = ctx
            .flow
            .nodes
            .get(&current)
            .ok_or_else(|| anyhow::anyhow!("node not found: {current}"))?;
        tracing::info!("node={}", current);

        if let Some(qa) = &node.qa {
            qa_node::run_qa(qa, &ctx.env, &mut state, &ctx.hbs).await?;
        }

        if let Some(tool) = &node.tool {
            payload =
                tool_node::run_tool(tool, &ctx.env, &state, ctx.tool_endpoint.as_str()).await?;
        }

        if let Some(tpl) = &node.template {
            let out = template_node::render_template(tpl, &ctx.hbs, &ctx.env, &state, &payload)?;
            let outmsg = OutMessage {
                ctx: ctx.tenant_ctx.clone(),
                tenant: ctx.env.tenant.clone(),
                platform: ctx.env.platform.clone(),
                chat_id: ctx.env.chat_id.clone(),
                thread_id: ctx.env.thread_id.clone(),
                kind: OutKind::Text,
                text: Some(out),
                message_card: None,
                adaptive_card: None,
                meta: Default::default(),
            };
            emit_pending_auth_telemetry(&outmsg);
            let team = ctx
                .tenant_ctx
                .team
                .as_ref()
                .map(|team| team.as_str())
                .unwrap_or("default");
            let subject = egress_subject(
                ctx.tenant_ctx.env.as_str(),
                ctx.tenant_ctx.tenant.as_str(),
                team,
                ctx.env.platform.as_str(),
            );
            ctx.nats
                .publish(subject, serde_json::to_vec(&outmsg)?.into())
                .await?;
        }

        if let Some(card) = &node.card {
            let card = card_node::render_card(card, &ctx.hbs, &ctx.env, &state, &payload)?;
            let outmsg = OutMessage {
                ctx: ctx.tenant_ctx.clone(),
                tenant: ctx.env.tenant.clone(),
                platform: ctx.env.platform.clone(),
                chat_id: ctx.env.chat_id.clone(),
                thread_id: ctx.env.thread_id.clone(),
                kind: OutKind::Card,
                text: None,
                message_card: Some(card),
                adaptive_card: None,
                meta: Default::default(),
            };
            emit_pending_auth_telemetry(&outmsg);
            let team = ctx
                .tenant_ctx
                .team
                .as_ref()
                .map(|team| team.as_str())
                .unwrap_or("default");
            let subject = egress_subject(
                ctx.tenant_ctx.env.as_str(),
                ctx.tenant_ctx.tenant.as_str(),
                team,
                ctx.env.platform.as_str(),
            );
            ctx.nats
                .publish(subject, serde_json::to_vec(&outmsg)?.into())
                .await?;
        }

        if let Some(next) = node.routes.first() {
            if next == "end" {
                break;
            }
            current = next.clone();
            continue;
        }
        break;
    }

    let session_data = SessionData {
        tenant_ctx: ctx.tenant_ctx.clone(),
        flow_id: flow_id(&ctx.flow.id)?,
        pack_id: ctx.pack_id,
        cursor: SessionCursor::new(current),
        context_json: serde_json::to_string(&state)?,
    };

    if let Some(existing_key) = previous_session {
        ctx.sessions
            .update_session(&existing_key, session_data)
            .await?;
    } else if active_user.is_some() {
        ctx.sessions
            .create_session(&ctx.tenant_ctx, session_data)
            .await?;
    } else {
        tracing::debug!("skipping session persistence; no user context available");
    }
    Ok(())
}

#[derive(Clone)]
struct ProcessContext {
    nats: Nats,
    flow_registry: Arc<FlowRegistry>,
    hbs: handlebars::Handlebars<'static>,
    sessions: SharedSessionStore,
    dlq: DlqPublisher,
    tool_endpoint: String,
}

async fn handle_env(ctx: Arc<ProcessContext>, channel: ChannelMessage) {
    let nats = ctx.nats.clone();
    let flow_entry = match ctx.flow_registry.select_flow(&channel) {
        Ok(flow) => flow,
        Err(err) => {
            tracing::error!(error = %err, "failed to select flow for channel message");
            return;
        }
    };
    let flow = flow_entry.flow.clone();
    let hbs = ctx.hbs.clone();
    let sessions = ctx.sessions.clone();
    let tenant_ctx = channel.tenant.clone();
    set_current_tenant_ctx(tenant_ctx.clone());
    let env = match message_from_channel(&channel) {
        Ok(env) => env,
        Err(err) => {
            tracing::error!(error = %err, "failed to decode channel payload");
            return;
        }
    };

    tracing::info!(
        pack_id = %flow_entry.pack_id,
        flow_id = %flow_entry.flow_id,
        platform = %env.platform.as_str(),
        "selected flow for inbound message"
    );

    let pack_id = PackId::new(flow_entry.pack_id.as_str()).ok();
    if let Err(e) = run_one(RunOneContext {
        nats,
        flow,
        hbs,
        sessions,
        tenant_ctx: tenant_ctx.clone(),
        env: env.clone(),
        tool_endpoint: ctx.tool_endpoint.clone(),
        pack_id,
    })
    .await
    {
        tracing::error!("run failed: {e}");
        if let Err(err) = ctx
            .dlq
            .publish(
                tenant_ctx.tenant.as_str(),
                env.platform.as_str(),
                &env.msg_id,
                1,
                DlqError {
                    code: "E_TRANSLATE".into(),
                    message: e.to_string(),
                    stage: None,
                },
                &channel,
            )
            .await
        {
            tracing::error!("failed to publish dlq entry: {err}");
        }
    }
}

fn message_from_channel(channel: &ChannelMessage) -> Result<MessageEnvelope> {
    let platform = Platform::from_str(channel.channel_id.as_str())
        .map_err(|err| anyhow::anyhow!("invalid platform: {err}"))?;
    let payload = &channel.payload;
    let chat_id = payload
        .get("chat_id")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| channel.session_id.clone());
    if chat_id.trim().is_empty() {
        return Err(anyhow::anyhow!("channel message missing chat_id"));
    }
    let msg_id = payload
        .get("msg_id")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let timestamp = payload
        .get("timestamp")
        .and_then(|v| v.as_str())
        .unwrap_or("1970-01-01T00:00:00Z")
        .to_string();
    let user_id = payload
        .get("user_id")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let thread_id = payload
        .get("thread_id")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let text = payload
        .get("text")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    let mut context = std::collections::BTreeMap::new();
    if let Some(meta) = payload.get("metadata").and_then(|v| v.as_object()) {
        for (k, v) in meta {
            context.insert(k.clone(), v.clone());
        }
    }
    if let Some(headers) = payload.get("headers") {
        context.insert("headers".into(), headers.clone());
    }

    Ok(MessageEnvelope {
        tenant: channel.tenant.tenant.as_str().to_string(),
        platform,
        chat_id,
        user_id,
        thread_id,
        msg_id,
        text,
        timestamp,
        context,
    })
}

fn emit_pending_auth_telemetry(out: &OutMessage) {
    if let Some(card) = out.adaptive_card.as_ref()
        && matches!(card.kind, gsm_core::messaging_card::MessageCardKind::Oauth)
        && let Some(oauth) = card.oauth.as_ref()
    {
        let labels = TelemetryLabels {
            tenant: out.tenant.clone(),
            platform: Some(out.platform.as_str().to_string()),
            chat_id: Some(out.chat_id.clone()),
            msg_id: Some(out.message_id()),
            extra: Vec::new(),
        };
        let ctx = MessageContext::new(labels);
        let team = out.ctx.team.as_ref().map(|team| team.as_ref());
        record_auth_card_render(
            &ctx,
            oauth.provider.as_str(),
            AuthRenderMode::Pending,
            oauth.connection_name.as_deref(),
            oauth.start_url.as_deref(),
            team,
        );
    }
}

fn flow_id(raw: &str) -> Result<FlowId> {
    FlowId::try_from(raw).map_err(|err| anyhow::anyhow!(err))
}
