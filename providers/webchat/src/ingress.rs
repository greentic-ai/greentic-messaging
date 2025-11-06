use anyhow::{Context, Result};
use async_trait::async_trait;
use greentic_types::TenantCtx;
use metrics::{counter, histogram};
use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;
use std::time::Instant;
use time::OffsetDateTime;
use tracing::warn;

use crate::{
    activity_bridge::normalize_activity,
    backoff,
    bus::{SharedBus, Subject},
    circuit::{CircuitBreaker, CircuitLabels, CircuitSettings},
    session::{SharedSessionStore, WebchatSession},
    telemetry,
    types::GreenticEvent,
};

#[derive(Clone, Debug, Deserialize)]
pub struct ActivitiesEnvelope {
    #[serde(default)]
    pub activities: Vec<Value>,
    #[serde(default)]
    pub watermark: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ActivitiesTransportResponse {
    pub status: StatusCode,
    pub body: Option<ActivitiesEnvelope>,
}

#[async_trait]
pub trait ActivitiesTransport: Send + Sync {
    async fn poll(
        &self,
        dl_base: &str,
        conversation_id: &str,
        token: &str,
        watermark: Option<&str>,
    ) -> Result<ActivitiesTransportResponse>;
}

pub type SharedActivitiesTransport = Arc<dyn ActivitiesTransport>;

#[derive(Clone)]
pub struct ReqwestActivitiesTransport {
    client: reqwest::Client,
}

impl ReqwestActivitiesTransport {
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ActivitiesTransport for ReqwestActivitiesTransport {
    async fn poll(
        &self,
        dl_base: &str,
        conversation_id: &str,
        token: &str,
        watermark: Option<&str>,
    ) -> Result<ActivitiesTransportResponse> {
        let url = format!(
            "{}/conversations/{}/activities",
            dl_base.trim_end_matches('/'),
            conversation_id
        );

        let mut request = self.client.get(url).bearer_auth(token);
        if let Some(wm) = watermark {
            request = request.query(&[("watermark", wm)]);
        }

        let response = request.send().await?;

        let status = response.status();
        if status == StatusCode::OK {
            let body = response
                .json::<ActivitiesEnvelope>()
                .await
                .context("failed to decode activities envelope")?;
            Ok(ActivitiesTransportResponse {
                status,
                body: Some(body),
            })
        } else {
            Ok(ActivitiesTransportResponse { status, body: None })
        }
    }
}

#[derive(Clone)]
pub struct Ingress {
    bus: SharedBus,
    sessions: SharedSessionStore,
}

impl Ingress {
    pub fn new(bus: SharedBus, sessions: SharedSessionStore) -> Self {
        Self { bus, sessions }
    }

    pub async fn publish_incoming(
        &self,
        activity: &Value,
        tenant_ctx: &TenantCtx,
        conversation_id: &str,
    ) -> Result<Option<GreenticEvent>> {
        let mut session = match self.sessions.get(conversation_id).await? {
            Some(existing) => existing,
            None => {
                let session = WebchatSession::new(
                    conversation_id.to_string(),
                    tenant_ctx.clone(),
                    String::new(),
                );
                self.sessions.upsert(session.clone()).await?;
                session
            }
        };

        if let Some(message) = normalize_activity(&session, activity) {
            let subject = Subject::incoming(
                tenant_ctx.env.as_ref(),
                tenant_ctx.tenant.as_ref(),
                tenant_ctx.team.as_ref().map(|team| team.as_ref()),
            );
            let event = GreenticEvent::IncomingMessage(message.clone());
            let span = telemetry::span_for_activity(
                "activity.publish",
                tenant_ctx,
                conversation_id,
                &message.id,
            );
            let _guard = span.enter();
            self.bus.publish(&subject, &event).await?;
            session.last_seen_at = OffsetDateTime::now_utc();
            self.sessions.upsert(session).await?;
            Ok(Some(event))
        } else {
            Ok(None)
        }
    }
}

#[derive(Clone)]
pub struct IngressDeps {
    pub bus: SharedBus,
    pub sessions: SharedSessionStore,
    pub dl_base: String,
    pub transport: SharedActivitiesTransport,
    pub circuit: CircuitSettings,
}

#[derive(Clone)]
pub struct IngressCtx {
    pub tenant_ctx: TenantCtx,
    pub conversation_id: String,
    pub token: String,
}

pub async fn run_poll_loop(deps: IngressDeps, ctx: IngressCtx) -> Result<()> {
    let mut attempt: u32 = 0;
    let ingress = Ingress::new(Arc::clone(&deps.bus), Arc::clone(&deps.sessions));

    let mut session = match deps.sessions.get(&ctx.conversation_id).await? {
        Some(mut existing) => {
            if existing.bearer_token != ctx.token {
                deps.sessions
                    .update_bearer_token(&ctx.conversation_id, ctx.token.clone())
                    .await?;
                existing.bearer_token = ctx.token.clone();
            }
            existing
        }
        None => {
            let new_session = WebchatSession::new(
                ctx.conversation_id.clone(),
                ctx.tenant_ctx.clone(),
                ctx.token.clone(),
            );
            deps.sessions.upsert(new_session.clone()).await?;
            new_session
        }
    };

    let (env_label, tenant_label, team_label) = telemetry::tenant_labels(&ctx.tenant_ctx);
    let env_metric = env_label.to_string();
    let tenant_metric = tenant_label.to_string();
    let team_metric = team_label.to_string();
    let mut circuit = CircuitBreaker::new(
        deps.circuit.clone(),
        CircuitLabels::new(
            env_label.to_string(),
            tenant_label.to_string(),
            team_label.to_string(),
            ctx.conversation_id.clone(),
        ),
    );

    let poll_span =
        telemetry::span_for_conversation("poll.loop", &ctx.tenant_ctx, &ctx.conversation_id);
    let _poll_guard = poll_span.enter();

    loop {
        circuit.before_request().await;

        let poll_started = Instant::now();
        let response = match deps
            .transport
            .poll(
                deps.dl_base.as_str(),
                ctx.conversation_id.as_str(),
                ctx.token.as_str(),
                session.watermark.as_deref(),
            )
            .await
        {
            Ok(resp) => resp,
            Err(err) => {
                counter!(
                    "webchat_errors_total",
                    "kind" => "transport",
                    "env" => env_metric.clone(),
                    "tenant" => tenant_metric.clone(),
                    "team" => team_metric.clone(),
                    "conversation" => ctx.conversation_id.clone(),
                )
                .increment(1);
                warn!(error = %err, "webchat poll transport error");
                circuit.on_failure();
                attempt = attempt.saturating_add(1);
                backoff::sleep(attempt).await;
                continue;
            }
        };

        let latency = poll_started.elapsed().as_secs_f64();
        let status_label = response.status.as_str().to_string();
        histogram!(
            "webchat_poll_latency_seconds",
            "env" => env_metric.clone(),
            "tenant" => tenant_metric.clone(),
            "team" => team_metric.clone(),
            "conversation" => ctx.conversation_id.clone(),
            "status" => status_label
        )
        .record(latency);

        match response.status {
            StatusCode::OK => {
                circuit.on_success();
                attempt = 0;

                let body = response
                    .body
                    .context("missing activities body for ok response")?;

                for activity in body.activities.iter() {
                    counter!(
                        "webchat_activities_polled_total",
                        "env" => env_metric.clone(),
                        "tenant" => tenant_metric.clone(),
                        "team" => team_metric.clone(),
                        "conversation" => ctx.conversation_id.clone()
                    )
                    .increment(1);

                    if ingress
                        .publish_incoming(activity, &ctx.tenant_ctx, &ctx.conversation_id)
                        .await?
                        .is_some()
                    {
                        counter!(
                            "webchat_activities_published_total",
                            "env" => env_metric.clone(),
                            "tenant" => tenant_metric.clone(),
                            "team" => team_metric.clone(),
                            "conversation" => ctx.conversation_id.clone()
                        )
                        .increment(1);
                    }
                }

                if body.watermark != session.watermark {
                    session.watermark = body.watermark.clone();
                    session.last_seen_at = OffsetDateTime::now_utc();
                    deps.sessions
                        .update_watermark(&ctx.conversation_id, body.watermark.clone())
                        .await?;
                }
            }
            StatusCode::TOO_MANY_REQUESTS
            | StatusCode::BAD_GATEWAY
            | StatusCode::SERVICE_UNAVAILABLE
            | StatusCode::INTERNAL_SERVER_ERROR => {
                let error_kind = response.status.as_str().to_string();
                counter!(
                    "webchat_errors_total",
                    "kind" => error_kind,
                    "env" => env_metric.clone(),
                    "tenant" => tenant_metric.clone(),
                    "team" => team_metric.clone(),
                    "conversation" => ctx.conversation_id.clone(),
                )
                .increment(1);
                circuit.on_failure();
                attempt = attempt.saturating_add(1);
                backoff::sleep(attempt).await;
                continue;
            }
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN | StatusCode::NOT_FOUND => {
                circuit.on_success();
                warn!(status = ?response.status, "webchat poll terminated: token or conversation invalid");
                break;
            }
            status => {
                let status_label = status.as_str().to_string();
                counter!(
                    "webchat_errors_total",
                    "kind" => status_label,
                    "env" => env_metric.clone(),
                    "tenant" => tenant_metric.clone(),
                    "team" => team_metric.clone(),
                    "conversation" => ctx.conversation_id.clone(),
                )
                .increment(1);
                warn!(?status, "webchat poll encountered unrecoverable status");
                break;
            }
        }
    }

    Ok(())
}
