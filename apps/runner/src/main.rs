mod card_node;
mod model;
mod qa_node;
mod session;
mod template_node;
mod tool_node;

use anyhow::Result;
use async_nats::Client as Nats;
use futures::StreamExt;
use greentic_types::session::SessionKey;
use gsm_core::*;
use gsm_dlq::{DlqError, DlqPublisher, replay_subject};
use gsm_session::{SessionRecord, SessionSnapshot, SharedSessionStore, store_from_env};
use gsm_telemetry::{
    AuthRenderMode, MessageContext, TelemetryLabels, install as init_telemetry,
    record_auth_card_render, set_current_tenant_ctx,
};
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

#[tokio::main]
async fn main() -> Result<()> {
    init_telemetry("greentic-messaging")?;
    let nats_url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
    let tenant = std::env::var("TENANT").unwrap_or_else(|_| "acme".into());
    let platform = std::env::var("PLATFORM").unwrap_or_else(|_| "telegram".into());
    let chat_prefix = std::env::var("CHAT_PREFIX").unwrap_or_else(|_| ">".into());
    let flow_path =
        std::env::var("FLOW").unwrap_or_else(|_| "examples/flows/weather_telegram.yaml".into());

    let flow = model::Flow::load_from_file(&flow_path)?;
    tracing::info!("Loaded flow id={} entry={}", flow.id, flow.r#in);

    let nats = async_nats::connect(nats_url).await?;
    let dlq = DlqPublisher::new("translate", nats.clone()).await?;

    let subject = format!("greentic.msg.in.{tenant}.{platform}.{chat_prefix}");
    let mut sub = nats.subscribe(subject.clone()).await?;
    tracing::info!("runner subscribed to {subject}");

    let replay_subject = replay_subject(&tenant, "translate");
    let mut replay_sub = nats.subscribe(replay_subject.clone()).await?;
    tracing::info!("runner subscribed to {replay_subject} for replays");

    let hbs = template_node::hb_registry();
    let sessions = store_from_env().await?;

    let ctx = Arc::new(ProcessContext {
        nats: nats.clone(),
        flow: flow.clone(),
        hbs: hbs.clone(),
        sessions: sessions.clone(),
        dlq: dlq.clone(),
    });

    {
        let ctx = Arc::clone(&ctx);
        tokio::spawn(async move {
            while let Some(msg) = replay_sub.next().await {
                match serde_json::from_slice::<InvocationEnvelope>(&msg.payload) {
                    Ok(inv) => handle_env(ctx.clone(), inv).await,
                    Err(e) => tracing::warn!("bad replay envelope: {e}"),
                }
            }
        });
    }

    while let Some(msg) = sub.next().await {
        let invocation: InvocationEnvelope = match serde_json::from_slice(&msg.payload) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("bad invocation envelope: {e}");
                continue;
            }
        };
        let ctx = Arc::clone(&ctx);
        tokio::spawn(async move {
            handle_env(ctx, invocation).await;
        });
    }

    Ok(())
}

async fn run_one(
    nats: Nats,
    flow: model::Flow,
    hbs: handlebars::Handlebars<'static>,
    sessions: SharedSessionStore,
    tenant_ctx: TenantCtx,
    env: MessageEnvelope,
) -> Result<()> {
    let scope = session::scope_from(&tenant_ctx, &env);
    let existing = sessions.find_by_scope(&scope).await?;
    let (session_key, mut state) = if let Some(record) = existing {
        match serde_json::from_str::<serde_json::Value>(&record.snapshot.context_json) {
            Ok(value) if value.is_object() => (record.key, value),
            Ok(_) => (record.key, json!({})),
            Err(err) => {
                tracing::warn!(error = %err, "failed to parse stored session context");
                (record.key, json!({}))
            }
        }
    } else {
        (SessionKey::new(Uuid::new_v4().to_string()), json!({}))
    };

    let mut current = flow.r#in.clone();
    let mut payload: serde_json::Value = serde_json::json!({});

    loop {
        let node = flow
            .nodes
            .get(&current)
            .ok_or_else(|| anyhow::anyhow!("node not found: {current}"))?;
        tracing::info!("node={}", current);

        if let Some(qa) = &node.qa {
            qa_node::run_qa(qa, &env, &mut state, &hbs).await?;
        }

        if let Some(tool) = &node.tool {
            payload = tool_node::run_tool(tool, &env, &state).await?;
        }

        if let Some(tpl) = &node.template {
            let out = template_node::render_template(tpl, &hbs, &env, &state, &payload)?;
            let outmsg = OutMessage {
                ctx: tenant_ctx.clone(),
                tenant: env.tenant.clone(),
                platform: env.platform.clone(),
                chat_id: env.chat_id.clone(),
                thread_id: env.thread_id.clone(),
                kind: OutKind::Text,
                text: Some(out),
                message_card: None,
                adaptive_card: None,
                meta: Default::default(),
            };
            emit_pending_auth_telemetry(&outmsg);
            let subject = out_subject(&env.tenant, env.platform.as_str(), &env.chat_id);
            nats.publish(subject, serde_json::to_vec(&outmsg)?.into())
                .await?;
        }

        if let Some(card) = &node.card {
            let card = card_node::render_card(card, &hbs, &env, &state, &payload)?;
            let outmsg = OutMessage {
                ctx: tenant_ctx.clone(),
                tenant: env.tenant.clone(),
                platform: env.platform.clone(),
                chat_id: env.chat_id.clone(),
                thread_id: env.thread_id.clone(),
                kind: OutKind::Card,
                text: None,
                message_card: Some(card),
                adaptive_card: None,
                meta: Default::default(),
            };
            emit_pending_auth_telemetry(&outmsg);
            let subject = out_subject(&env.tenant, env.platform.as_str(), &env.chat_id);
            nats.publish(subject, serde_json::to_vec(&outmsg)?.into())
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

    let snapshot = SessionSnapshot {
        tenant_ctx: tenant_ctx.clone(),
        flow_id: flow.id.clone(),
        cursor_node: current,
        context_json: serde_json::to_string(&state)?,
    };
    let record = SessionRecord::new(session_key, scope, snapshot);
    sessions.save(record).await?;
    Ok(())
}

#[derive(Clone)]
struct ProcessContext {
    nats: Nats,
    flow: model::Flow,
    hbs: handlebars::Handlebars<'static>,
    sessions: SharedSessionStore,
    dlq: DlqPublisher,
}

async fn handle_env(ctx: Arc<ProcessContext>, invocation: InvocationEnvelope) {
    let nats = ctx.nats.clone();
    let flow = ctx.flow.clone();
    let hbs = ctx.hbs.clone();
    let sessions = ctx.sessions.clone();
    let tenant_ctx = invocation.ctx.clone();
    set_current_tenant_ctx(tenant_ctx.clone());
    let env = match MessageEnvelope::try_from(invocation.clone()) {
        Ok(env) => env,
        Err(err) => {
            tracing::error!(error = %err, "failed to decode invocation payload");
            return;
        }
    };

    if let Err(e) = run_one(nats, flow, hbs, sessions, tenant_ctx.clone(), env.clone()).await {
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
                &invocation,
            )
            .await
        {
            tracing::error!("failed to publish dlq entry: {err}");
        }
    }
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
