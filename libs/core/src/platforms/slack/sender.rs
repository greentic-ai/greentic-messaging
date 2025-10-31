use std::sync::Arc;

use async_trait::async_trait;
use reqwest::StatusCode;
use serde_json::{json, Value};

use crate::egress::{EgressSender, OutboundMessage, SendResult};
use crate::platforms::slack::workspace::{SlackWorkspace, SlackWorkspaceIndex};
use crate::prelude::*;
use crate::secrets_paths::{slack_workspace_index, slack_workspace_secret};

pub struct SlackSender<R>
where
    R: SecretsResolver + Send + Sync,
{
    http: reqwest::Client,
    secrets: Arc<R>,
    api_base: String,
}

impl<R> SlackSender<R>
where
    R: SecretsResolver + Send + Sync,
{
    pub fn new(http: reqwest::Client, secrets: Arc<R>, api_base: Option<String>) -> Self {
        Self {
            http,
            secrets,
            api_base: api_base.unwrap_or_else(|| "https://slack.com/api".into()),
        }
    }

    async fn workspace(&self, ctx: &TenantCtx) -> NodeResult<SlackWorkspace> {
        let team = ctx
            .team
            .as_ref()
            .ok_or_else(|| NodeError::new("slack_missing_team", "team required for slack"))?;
        self.load_workspace(ctx, team.as_str()).await
    }

    async fn load_workspace(&self, ctx: &TenantCtx, team: &str) -> NodeResult<SlackWorkspace> {
        let index_path = slack_workspace_index(ctx);
        if let Some(index) = self
            .secrets
            .get_json::<SlackWorkspaceIndex>(&index_path, ctx)
            .await?
        {
            for workspace_id in &index.workspaces {
                let path = slack_workspace_secret(ctx, workspace_id);
                if let Some(creds) = self.secrets.get_json(&path, ctx).await? {
                    return Ok(creds);
                }
            }
        }

        let fallback_path = slack_workspace_secret(ctx, team);
        if let Some(creds) = self.secrets.get_json(&fallback_path, ctx).await? {
            return Ok(creds);
        }

        Err(self.fail(
            "slack_missing_creds",
            format!(
                "no slack workspace creds under /{}/messaging/slack/{}/{}/workspace",
                ctx.env.0, ctx.tenant.0, team
            ),
        ))
    }

    fn ensure_payload(
        &self,
        mut payload: Value,
        channel: &str,
        text: Option<&str>,
    ) -> NodeResult<Value> {
        let obj = payload.as_object_mut().ok_or_else(|| {
            NodeError::new("slack_payload_not_object", "payload must be JSON object")
        })?;
        obj.entry("channel")
            .or_insert_with(|| Value::String(channel.to_string()));
        if let Some(text) = text {
            if !obj.contains_key("text") && !obj.contains_key("blocks") {
                obj.insert("text".into(), Value::String(text.to_string()));
            }
        }
        Ok(payload)
    }

    fn fail(&self, code: &str, message: impl Into<String>) -> NodeError {
        NodeError::new(code, message)
    }

    fn net(&self, err: reqwest::Error) -> NodeError {
        NodeError::new("slack_transport", err.to_string())
            .with_retry(Some(1_000))
            .with_source(err)
    }

    fn build_url(&self, path: &str) -> String {
        format!(
            "{}/{}",
            self.api_base.trim_end_matches('/'),
            path.trim_start_matches('/')
        )
    }
}

#[async_trait]
impl<R> EgressSender for SlackSender<R>
where
    R: SecretsResolver + Send + Sync,
{
    async fn send(&self, ctx: &TenantCtx, msg: OutboundMessage) -> NodeResult<SendResult> {
        let channel = msg
            .channel
            .as_deref()
            .ok_or_else(|| self.fail("slack_missing_channel", "channel missing"))?;

        let workspace = self.workspace(ctx).await?;

        if self.api_base.starts_with("mock://") {
            return Ok(SendResult {
                message_id: Some(workspace.workspace_id),
                raw: Some(json!({
                    "channel": channel,
                    "text": msg.text,
                    "payload": msg.payload,
                })),
            });
        }

        let payload = if let Some(body) = msg.payload.clone() {
            self.ensure_payload(body, channel, msg.text.as_deref())?
        } else {
            json!({
                "channel": channel,
                "text": msg.text.unwrap_or_default(),
            })
        };

        let url = self.build_url("chat.postMessage");
        let response = self
            .http
            .post(url)
            .bearer_auth(&workspace.bot_token)
            .json(&payload)
            .send()
            .await
            .map_err(|err| self.net(err))?;

        let status = response.status();
        let retry_header = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
            .map(|s| s * 1000);
        let body_text = response.text().await.map_err(|err| self.net(err))?;
        let raw: Value = serde_json::from_str(&body_text).unwrap_or(Value::Null);

        if !status.is_success() {
            let mut err = self
                .fail(
                    "slack_send_failed",
                    format!("status={} body={}", status.as_u16(), body_text),
                )
                .with_detail_text(
                    serde_json::to_string(&json!({
                        "status": status.as_u16(),
                        "body": body_text,
                    }))
                    .unwrap_or_else(|_| "{\"error\":\"failed to encode details\"}".to_string()),
                );
            if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
                err = err.with_retry(retry_header.or(Some(1_000)));
            }
            return Err(err);
        }

        let ok = raw.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
        if !ok {
            let error = raw
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let mut err =
                self.fail("slack_send_failed", error.to_string())
                    .with_detail_text(serde_json::to_string(&raw).unwrap_or_else(|_| {
                        "{\"error\":\"failed to encode details\"}".to_string()
                    }));
            if error == "ratelimited" {
                err = err.with_retry(retry_header.or(Some(1_000)));
            }
            return Err(err);
        }

        let message_id = raw
            .get("ts")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        Ok(SendResult {
            message_id,
            raw: Some(raw),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::make_tenant_ctx;
    use crate::secrets_paths::{slack_workspace_index, slack_workspace_secret};
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[derive(Default)]
    struct InMemorySecrets {
        store: Mutex<HashMap<String, Value>>,
    }

    #[async_trait]
    impl SecretsResolver for InMemorySecrets {
        async fn get_json<T>(&self, path: &SecretPath, _ctx: &TenantCtx) -> NodeResult<Option<T>>
        where
            T: serde::de::DeserializeOwned + Send,
        {
            let value = self.store.lock().unwrap().get(path.as_str()).cloned();
            if let Some(json) = value {
                Ok(Some(serde_json::from_value(json).map_err(|err| {
                    NodeError::new("decode", "failed to decode secret").with_source(err)
                })?))
            } else {
                Ok(None)
            }
        }

        async fn put_json<T>(
            &self,
            path: &SecretPath,
            _ctx: &TenantCtx,
            value: &T,
        ) -> NodeResult<()>
        where
            T: serde::Serialize + Sync + Send,
        {
            let json = serde_json::to_value(value).map_err(|err| {
                NodeError::new("encode", "failed to encode secret").with_source(err)
            })?;
            self.store
                .lock()
                .unwrap()
                .insert(path.as_str().to_string(), json);
            Ok(())
        }
    }

    #[tokio::test]
    async fn fetches_workspace_token_per_context() {
        let prev_env = std::env::var("GREENTIC_ENV").ok();
        std::env::set_var("GREENTIC_ENV", "test");

        let secrets = Arc::new(InMemorySecrets::default());
        let ctx_a = make_tenant_ctx("tenant-a".into(), Some("team-a".into()), None);
        let ctx_b = make_tenant_ctx("tenant-b".into(), Some("team-b".into()), None);

        let workspace_a = "T-A";
        let path_a = slack_workspace_secret(&ctx_a, workspace_a);
        secrets
            .put_json(
                &path_a,
                &ctx_a,
                &SlackWorkspace::new(workspace_a, "token-a"),
            )
            .await
            .unwrap();
        let index_a = slack_workspace_index(&ctx_a);
        secrets
            .put_json(
                &index_a,
                &ctx_a,
                &SlackWorkspaceIndex {
                    workspaces: vec![workspace_a.to_string()],
                },
            )
            .await
            .unwrap();

        let workspace_b_primary = "T-B2";
        let workspace_b_secondary = "T-B1";
        let path_b_primary = slack_workspace_secret(&ctx_b, workspace_b_primary);
        secrets
            .put_json(
                &path_b_primary,
                &ctx_b,
                &SlackWorkspace::new(workspace_b_primary, "token-b2"),
            )
            .await
            .unwrap();
        let path_b_secondary = slack_workspace_secret(&ctx_b, workspace_b_secondary);
        secrets
            .put_json(
                &path_b_secondary,
                &ctx_b,
                &SlackWorkspace::new(workspace_b_secondary, "token-b1"),
            )
            .await
            .unwrap();
        let index_b = slack_workspace_index(&ctx_b);
        secrets
            .put_json(
                &index_b,
                &ctx_b,
                &SlackWorkspaceIndex {
                    workspaces: vec![
                        workspace_b_primary.to_string(),
                        workspace_b_secondary.to_string(),
                    ],
                },
            )
            .await
            .unwrap();

        let sender = SlackSender::new(reqwest::Client::new(), secrets, Some("mock://slack".into()));

        let res_a = sender
            .send(
                &ctx_a,
                OutboundMessage {
                    channel: Some("C123".into()),
                    text: Some("hello".into()),
                    payload: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(res_a.message_id.as_deref(), Some(workspace_a));

        let res_b = sender
            .send(
                &ctx_b,
                OutboundMessage {
                    channel: Some("C456".into()),
                    text: Some("hola".into()),
                    payload: Some(json!({"text": "hola"})),
                },
            )
            .await
            .unwrap();
        assert_eq!(res_b.message_id.as_deref(), Some(workspace_b_primary));

        if let Some(env) = prev_env {
            std::env::set_var("GREENTIC_ENV", env);
        } else {
            std::env::remove_var("GREENTIC_ENV");
        }
    }

    #[test]
    fn workspace_secrets_and_urls_are_scoped() {
        let ctx_a = make_tenant_ctx("acme".into(), Some("support".into()), None);
        let ctx_b = make_tenant_ctx("globex".into(), Some("sales".into()), None);
        let path_a = slack_workspace_secret(&ctx_a, "T111");
        let path_b = slack_workspace_secret(&ctx_b, "T222");
        assert!(path_a.as_str().contains("/acme/"));
        assert!(path_b.as_str().contains("/globex/"));
        assert_ne!(path_a.as_str(), path_b.as_str());

        let secrets = Arc::new(InMemorySecrets::default());
        let sender_a = SlackSender::new(
            reqwest::Client::new(),
            secrets.clone(),
            Some("https://slack.com/api".into()),
        );
        let sender_b = SlackSender::new(
            reqwest::Client::new(),
            secrets,
            Some("https://globex.slackproxy.com/api".into()),
        );

        assert_eq!(
            sender_a.build_url("chat.postMessage"),
            "https://slack.com/api/chat.postMessage"
        );
        assert_eq!(
            sender_b.build_url("chat.postMessage"),
            "https://globex.slackproxy.com/api/chat.postMessage"
        );
    }

    #[tokio::test]
    async fn falls_back_to_team_workspace_without_index() {
        let prev_env = std::env::var("GREENTIC_ENV").ok();
        std::env::set_var("GREENTIC_ENV", "test");

        let secrets = Arc::new(InMemorySecrets::default());
        let ctx = make_tenant_ctx("tenant".into(), Some("team".into()), None);
        let path = slack_workspace_secret(&ctx, "team");
        secrets
            .put_json(&path, &ctx, &SlackWorkspace::new("team", "token"))
            .await
            .unwrap();

        let sender = SlackSender::new(reqwest::Client::new(), secrets, Some("mock://slack".into()));
        let res = sender
            .send(
                &ctx,
                OutboundMessage {
                    channel: Some("C1".into()),
                    text: Some("hi".into()),
                    payload: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(res.message_id.as_deref(), Some("team"));

        if let Some(env) = prev_env {
            std::env::set_var("GREENTIC_ENV", env);
        } else {
            std::env::remove_var("GREENTIC_ENV");
        }
    }

    #[tokio::test]
    async fn requires_team_context() {
        let secrets = Arc::new(InMemorySecrets::default());
        let sender = SlackSender::new(reqwest::Client::new(), secrets, Some("mock://slack".into()));
        let ctx = make_tenant_ctx("acme".into(), None, None);
        let err = sender
            .send(
                &ctx,
                OutboundMessage {
                    channel: Some("C1".into()),
                    text: Some("hi".into()),
                    payload: None,
                },
            )
            .await
            .expect_err("missing team");
        assert_eq!(
            err.to_string(),
            "slack_missing_team: team required for slack"
        );
    }

    #[tokio::test]
    async fn requires_channel() {
        let prev_env = std::env::var("GREENTIC_ENV").ok();
        std::env::set_var("GREENTIC_ENV", "test");
        let secrets = Arc::new(InMemorySecrets::default());
        let ctx = make_tenant_ctx("acme".into(), Some("team".into()), None);
        let path = slack_workspace_secret(&ctx, "team");
        secrets
            .put_json(&path, &ctx, &SlackWorkspace::new("team", "token"))
            .await
            .unwrap();
        let sender = SlackSender::new(reqwest::Client::new(), secrets, Some("mock://slack".into()));
        let err = sender
            .send(
                &ctx,
                OutboundMessage {
                    channel: None,
                    text: Some("hi".into()),
                    payload: None,
                },
            )
            .await
            .expect_err("missing channel");
        assert_eq!(err.to_string(), "slack_missing_channel: channel missing");
        if let Some(env) = prev_env {
            std::env::set_var("GREENTIC_ENV", env);
        } else {
            std::env::remove_var("GREENTIC_ENV");
        }
    }
}
