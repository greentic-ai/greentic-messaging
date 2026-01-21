use anyhow::Result;
use async_trait::async_trait;
use greentic_types::{FlowId, PackId, SessionCursor, SessionKey, UserId};
use gsm_core::{
    ChannelMessage, MessageEnvelope, OutKind, OutMessage, Platform, TenantCtx, egress_subject,
};
use gsm_session::{SessionData, SharedSessionStore};
use gsm_telemetry::set_current_tenant_ctx;
use serde_json::{Value, json};
use std::str::FromStr;

use crate::model::Flow;
use crate::{card_node, qa_node, template_node, tool_node};

#[derive(Clone, Copy, Debug)]
pub enum ToolMode {
    Live,
    Stub,
}

#[derive(Clone, Debug)]
pub struct ExecutionOptions {
    pub tool_mode: ToolMode,
    pub allow_agent: bool,
    pub tool_endpoint: String,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct ToolCall {
    pub tool: String,
    pub action: String,
    pub input: Value,
}

#[derive(Debug)]
pub struct RunnerOutcome {
    pub out_messages: Vec<OutMessage>,
    pub tool_calls: Vec<ToolCall>,
    pub state: Value,
}

#[async_trait]
pub trait RunnerSink: Send + Sync {
    async fn publish_out_message(&self, subject: &str, out: &OutMessage) -> Result<()>;
}

#[async_trait]
impl RunnerSink for async_nats::Client {
    async fn publish_out_message(&self, subject: &str, out: &OutMessage) -> Result<()> {
        self.publish(subject.to_string(), serde_json::to_vec(out)?.into())
            .await?;
        Ok(())
    }
}

pub fn message_from_channel(channel: &ChannelMessage) -> Result<MessageEnvelope> {
    let platform = Platform::from_str(channel.channel_id.as_str())
        .map_err(|err| anyhow::anyhow!("invalid platform: {err}"))?;
    let payload = &channel.payload;
    let chat_id = payload
        .get("chat_id")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| channel.session_id.clone());
    if chat_id.trim().is_empty() {
        anyhow::bail!("channel message missing chat_id");
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

#[allow(clippy::too_many_arguments)]
pub async fn run_flow(
    flow_id: &str,
    flow: &Flow,
    tenant_ctx: &TenantCtx,
    env: &MessageEnvelope,
    sessions: &SharedSessionStore,
    hbs: &handlebars::Handlebars<'static>,
    sink: &dyn RunnerSink,
    options: &ExecutionOptions,
    pack_id: Option<PackId>,
) -> Result<RunnerOutcome> {
    let active_user = tenant_ctx
        .user
        .clone()
        .or_else(|| tenant_ctx.user_id.clone())
        .or_else(|| UserId::try_from(env.user_id.as_str()).ok());
    let mut previous_session: Option<SessionKey> = None;
    let mut state = if let Some(user) = active_user.clone() {
        match sessions.find_by_user(tenant_ctx, &user).await {
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

    let mut current = flow.r#in.clone();
    let mut payload: serde_json::Value = serde_json::json!({});
    let mut out_messages = Vec::new();
    let mut tool_calls = Vec::new();

    loop {
        let node = flow
            .nodes
            .get(&current)
            .ok_or_else(|| anyhow::anyhow!("node not found: {current}"))?;
        tracing::info!("node={}", current);

        if let Some(qa) = &node.qa {
            if options.allow_agent {
                qa_node::run_qa(qa, env, &mut state, hbs).await?;
            } else {
                qa_node::run_qa_offline(qa, env, &mut state).await?;
            }
        }

        if let Some(tool) = &node.tool {
            let input = tool_node::render_tool_input(tool, env, &state)?;
            tool_calls.push(ToolCall {
                tool: tool.tool.clone(),
                action: tool.action.clone(),
                input: input.clone(),
            });
            payload = match options.tool_mode {
                ToolMode::Live => {
                    tool_node::run_tool_with_input(tool, input, options.tool_endpoint.as_str())
                        .await?
                }
                ToolMode::Stub => tool_node::run_tool_stub_with_input(input)?,
            };
        }

        if let Some(tpl) = &node.template {
            let out = template_node::render_template(tpl, hbs, env, &state, &payload)?;
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
                meta: env.context.clone(),
            };
            let team = tenant_ctx
                .team
                .as_ref()
                .map(|team| team.as_str())
                .unwrap_or("default");
            let subject = egress_subject(
                tenant_ctx.env.as_str(),
                tenant_ctx.tenant.as_str(),
                team,
                env.platform.as_str(),
            );
            sink.publish_out_message(&subject, &outmsg).await?;
            out_messages.push(outmsg);
        }

        if let Some(card) = &node.card {
            let card = card_node::render_card(card, hbs, env, &state, &payload)?;
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
                meta: env.context.clone(),
            };
            let team = tenant_ctx
                .team
                .as_ref()
                .map(|team| team.as_str())
                .unwrap_or("default");
            let subject = egress_subject(
                tenant_ctx.env.as_str(),
                tenant_ctx.tenant.as_str(),
                team,
                env.platform.as_str(),
            );
            sink.publish_out_message(&subject, &outmsg).await?;
            out_messages.push(outmsg);
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
        tenant_ctx: tenant_ctx.clone(),
        flow_id: FlowId::new(flow_id)?,
        pack_id,
        cursor: SessionCursor::new(current),
        context_json: serde_json::to_string(&state)?,
    };

    if let Some(existing_key) = previous_session {
        sessions.update_session(&existing_key, session_data).await?;
    } else if active_user.is_some() {
        sessions.create_session(tenant_ctx, session_data).await?;
    } else {
        tracing::debug!("skipping session persistence; no user context available");
    }

    Ok(RunnerOutcome {
        out_messages,
        tool_calls,
        state,
    })
}

pub fn set_tenant_ctx(ctx: TenantCtx) {
    set_current_tenant_ctx(ctx);
}

pub fn env_from_channel(channel: &ChannelMessage) -> Result<MessageEnvelope> {
    message_from_channel(channel)
}
