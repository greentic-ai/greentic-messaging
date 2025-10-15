mod card_node;
mod model;
mod qa_node;
mod session;
mod template_node;
mod tool_node;

use anyhow::Result;
use async_nats::Client as Nats;
use futures::StreamExt;
use gsm_core::*;
use gsm_dlq::{replay_subject, DlqError, DlqPublisher};
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();
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

    let subject = format!("greentic.msg.in.{}.{}.{}", tenant, platform, chat_prefix);
    let mut sub = nats.subscribe(subject.clone()).await?;
    tracing::info!("runner subscribed to {subject}");

    let replay_subject = replay_subject(&tenant, "translate");
    let mut replay_sub = nats.subscribe(replay_subject.clone()).await?;
    tracing::info!("runner subscribed to {replay_subject} for replays");

    let hbs = template_node::hb_registry();
    let sessions = session::Sessions::default();

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
                match serde_json::from_slice::<MessageEnvelope>(&msg.payload) {
                    Ok(env) => handle_env(ctx.clone(), env).await,
                    Err(e) => tracing::warn!("bad replay envelope: {e}"),
                }
            }
        });
    }

    while let Some(msg) = sub.next().await {
        let env: MessageEnvelope = match serde_json::from_slice(&msg.payload) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("bad envelope: {e}");
                continue;
            }
        };
        let ctx = Arc::clone(&ctx);
        tokio::spawn(async move {
            handle_env(ctx, env).await;
        });
    }

    Ok(())
}

async fn run_one(
    nats: Nats,
    flow: model::Flow,
    hbs: handlebars::Handlebars<'static>,
    sessions: session::Sessions,
    env: MessageEnvelope,
) -> Result<()> {
    let sid = session::SessionId::from_env(&env);
    let mut state = sessions.get(&sid);
    if !state.is_object() {
        state = serde_json::json!({});
    }

    let mut current = flow.r#in.clone();
    let mut payload: serde_json::Value = serde_json::json!({});

    loop {
        let node = flow
            .nodes
            .get(&current)
            .ok_or_else(|| anyhow::anyhow!("node not found: {}", current))?;
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
                tenant: env.tenant.clone(),
                platform: env.platform.clone(),
                chat_id: env.chat_id.clone(),
                thread_id: env.thread_id.clone(),
                kind: OutKind::Text,
                text: Some(out),
                message_card: None,
                meta: Default::default(),
            };
            let subject = out_subject(&env.tenant, env.platform.as_str(), &env.chat_id);
            nats.publish(subject, serde_json::to_vec(&outmsg)?.into())
                .await?;
        }

        if let Some(card) = &node.card {
            let card = card_node::render_card(card, &hbs, &env, &state, &payload)?;
            let outmsg = OutMessage {
                tenant: env.tenant.clone(),
                platform: env.platform.clone(),
                chat_id: env.chat_id.clone(),
                thread_id: env.thread_id.clone(),
                kind: OutKind::Card,
                text: None,
                message_card: Some(card),
                meta: Default::default(),
            };
            let subject = out_subject(&env.tenant, env.platform.as_str(), &env.chat_id);
            nats.publish(subject, serde_json::to_vec(&outmsg)?.into())
                .await?;
        }

        if let Some(next) = node.routes.get(0) {
            if next == "end" {
                break;
            }
            current = next.clone();
            continue;
        }
        break;
    }

    sessions.put(&sid, state);
    Ok(())
}

#[derive(Clone)]
struct ProcessContext {
    nats: Nats,
    flow: model::Flow,
    hbs: handlebars::Handlebars<'static>,
    sessions: session::Sessions,
    dlq: DlqPublisher,
}

async fn handle_env(ctx: Arc<ProcessContext>, env: MessageEnvelope) {
    let nats = ctx.nats.clone();
    let flow = ctx.flow.clone();
    let hbs = ctx.hbs.clone();
    let sessions = ctx.sessions.clone();
    if let Err(e) = run_one(nats, flow, hbs, sessions, env.clone()).await {
        tracing::error!("run failed: {e}");
        if let Err(err) = ctx
            .dlq
            .publish(
                &env.tenant,
                env.platform.as_str(),
                &env.msg_id,
                1,
                DlqError {
                    code: "E_TRANSLATE".into(),
                    message: e.to_string(),
                    stage: None,
                },
                &env,
            )
            .await
        {
            tracing::error!("failed to publish dlq entry: {err}");
        }
    }
}
