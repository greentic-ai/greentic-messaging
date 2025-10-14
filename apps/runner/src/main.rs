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

    let subject = format!("greentic.msg.in.{}.{}.{}", tenant, platform, chat_prefix);
    let mut sub = nats.subscribe(subject.clone()).await?;
    tracing::info!("runner subscribed to {subject}");

    let hbs = template_node::hb_registry();
    let sessions = session::Sessions::default();

    while let Some(msg) = sub.next().await {
        let env: MessageEnvelope = match serde_json::from_slice(&msg.payload) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("bad envelope: {e}");
                continue;
            }
        };
        let nats = nats.clone();
        let flow = flow.clone();
        let hbs = hbs.clone();
        let sessions = sessions.clone();
        tokio::spawn(async move {
            if let Err(e) = run_one(nats.clone(), flow, hbs, sessions, env.clone()).await {
                tracing::error!("run failed: {e}");
                let subject = format!(
                    "greentic.msg.dlq.in.{}.{}",
                    env.tenant,
                    env.platform.as_str()
                );
                let _ = nats
                    .publish(subject, serde_json::to_vec(&env).unwrap().into())
                    .await;
            }
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
