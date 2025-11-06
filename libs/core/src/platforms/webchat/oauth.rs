use anyhow::{Context, anyhow};
use async_trait::async_trait;
use axum::{
    extract::{Query, State},
    response::{Html, IntoResponse, Redirect, Response},
};
use greentic_types::TenantCtx;
use metrics::counter;
use reqwest::Client;
use serde::Deserialize;
use serde_json::{Value, json};
use tracing::warn;

#[cfg(feature = "directline_standalone")]
use super::conversation::{Activity, ChannelAccount, StoreError};
use super::{config::OAuthProviderConfig, error::WebChatError, http::AppState, telemetry};

pub fn contains_oauth_card(activity: &Value) -> bool {
    activity
        .get("attachments")
        .and_then(Value::as_array)
        .map(|attachments| {
            attachments.iter().any(|attachment| {
                attachment
                    .get("contentType")
                    .or_else(|| attachment.get("content_type"))
                    .and_then(Value::as_str)
                    .map(|ct| {
                        ct.to_ascii_lowercase()
                            .starts_with("application/vnd.microsoft.card.oauth")
                    })
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

#[derive(Debug, Deserialize)]
pub struct StartQuery {
    #[serde(rename = "conversationId")]
    pub conversation_id: String,
    #[serde(default)]
    pub state: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CallbackQuery {
    #[serde(rename = "conversationId")]
    pub conversation_id: String,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

pub async fn start(
    State(state): State<AppState>,
    Query(query): Query<StartQuery>,
) -> Result<impl IntoResponse, OAuthRouteError> {
    let session = state
        .sessions
        .get(&query.conversation_id)
        .await
        .map_err(OAuthRouteError::Storage)?
        .ok_or(OAuthRouteError::ConversationNotFound)?;

    let oauth_config = state
        .provider
        .oauth_config(&session.tenant_ctx)
        .await
        .map_err(|err| OAuthRouteError::Resolve(WebChatError::Internal(err)))?
        .ok_or(OAuthRouteError::NotConfigured)?;

    let redirect_uri = build_redirect_uri(&oauth_config, &query.conversation_id)?;
    let authorize_url = build_authorize_url(&oauth_config, &redirect_uri, query.state.as_deref())?;

    let (env_label, tenant_label, team_label) = telemetry::tenant_labels(&session.tenant_ctx);
    let env_metric = env_label.to_string();
    let tenant_metric = tenant_label.to_string();
    let team_metric = team_label.to_string();
    counter!(
        "webchat_oauth_started_total",
        "env" => env_metric.clone(),
        "tenant" => tenant_metric.clone(),
        "team" => team_metric.clone()
    )
    .increment(1);

    Ok(Redirect::temporary(authorize_url.as_str()))
}

pub async fn callback(
    State(state): State<AppState>,
    Query(query): Query<CallbackQuery>,
) -> Result<impl IntoResponse, OAuthRouteError> {
    if let Some(error) = &query.error {
        warn!(reason = error.as_str(), "oauth callback returned error");
        return Ok(Html(CLOSE_WINDOW_HTML));
    }

    let code = query
        .code
        .as_deref()
        .ok_or(OAuthRouteError::BadRequest("missing code"))?;

    let session = state
        .sessions
        .get(&query.conversation_id)
        .await
        .map_err(OAuthRouteError::Storage)?
        .ok_or(OAuthRouteError::ConversationNotFound)?;

    let oauth_config = state
        .provider
        .oauth_config(&session.tenant_ctx)
        .await
        .map_err(|err| OAuthRouteError::Resolve(WebChatError::Internal(err)))?
        .ok_or(OAuthRouteError::NotConfigured)?;
    let redirect_uri = build_redirect_uri(&oauth_config, &query.conversation_id)?;
    let token_handle = state
        .oauth_client
        .exchange_code(&session.tenant_ctx, &oauth_config, code, &redirect_uri)
        .await
        .map_err(OAuthRouteError::Exchange)?;

    #[cfg(feature = "directline_standalone")]
    {
        let mut activity = Activity::new("message");
        activity.text = Some("You're signed in.".to_string());
        activity.from = Some(ChannelAccount {
            id: "bot".into(),
            name: None,
            role: Some("bot".into()),
        });
        activity.channel_data = Some(json!({
            "oauth_token_handle": token_handle,
        }));
        let append_result = state
            .conversations
            .append(&session.conversation_id, activity.clone())
            .await;
        let stored = match append_result {
            Ok(stored) => stored,
            Err(StoreError::NotFound(_)) => {
                state
                    .conversations
                    .create(&session.conversation_id, session.tenant_ctx.clone())
                    .await
                    .map_err(|err| OAuthRouteError::Resume(WebChatError::Internal(err.into())))?;
                state
                    .conversations
                    .append(&session.conversation_id, activity)
                    .await
                    .map_err(|err| OAuthRouteError::Resume(WebChatError::Internal(err.into())))?
            }
            Err(StoreError::QuotaExceeded(_)) => {
                return Err(OAuthRouteError::Resume(WebChatError::BadRequest(
                    "conversation backlog quota exceeded",
                )));
            }
            Err(err) => return Err(OAuthRouteError::Resume(WebChatError::Internal(err.into()))),
        };

        if let Err(err) = state
            .sessions
            .update_watermark(
                &session.conversation_id,
                Some((stored.watermark + 1).to_string()),
            )
            .await
        {
            warn!(error = %err, "failed to update watermark after oauth");
        }
    }
    #[cfg(not(feature = "directline_standalone"))]
    {
        let activity = json!({
            "type": "event",
            "name": "oauth.token",
            "channelData": {
                "oauth_token_handle": token_handle,
            }
        });

        state
            .post_activity(
                &session.conversation_id,
                session.bearer_token.as_str(),
                activity,
            )
            .await
            .map_err(OAuthRouteError::Resume)?;
    }

    let (env_label, tenant_label, team_label) = telemetry::tenant_labels(&session.tenant_ctx);
    let env_metric = env_label.to_string();
    let tenant_metric = tenant_label.to_string();
    let team_metric = team_label.to_string();
    counter!(
        "webchat_oauth_completed_total",
        "env" => env_metric.clone(),
        "tenant" => tenant_metric.clone(),
        "team" => team_metric.clone()
    )
    .increment(1);

    Ok(Html(CLOSE_WINDOW_HTML))
}

fn build_redirect_uri(
    config: &OAuthProviderConfig,
    conversation_id: &str,
) -> Result<String, OAuthRouteError> {
    let mut redirect = reqwest::Url::parse(&format!(
        "{}/webchat/oauth/callback",
        config.redirect_base.trim_end_matches('/')
    ))
    .map_err(|err| OAuthRouteError::Url(err.into()))?;
    redirect
        .query_pairs_mut()
        .append_pair("conversationId", conversation_id);
    Ok(redirect.into())
}

fn build_authorize_url(
    config: &OAuthProviderConfig,
    redirect_uri: &str,
    state: Option<&str>,
) -> Result<reqwest::Url, OAuthRouteError> {
    let mut url = reqwest::Url::parse(&format!(
        "{}/authorize",
        config.issuer.trim_end_matches('/')
    ))
    .map_err(|err| OAuthRouteError::Url(err.into()))?;
    {
        let mut pairs = url.query_pairs_mut();
        pairs.append_pair("client_id", config.client_id.as_str());
        pairs.append_pair("response_type", "code");
        pairs.append_pair("redirect_uri", redirect_uri);
        if let Some(state) = state {
            pairs.append_pair("state", state);
        }
    }
    Ok(url)
}

#[derive(Debug)]
pub enum OAuthRouteError {
    BadRequest(&'static str),
    ConversationNotFound,
    NotConfigured,
    Url(anyhow::Error),
    Storage(anyhow::Error),
    Exchange(anyhow::Error),
    Resolve(WebChatError),
    Resume(WebChatError),
}

impl OAuthRouteError {
    fn as_status(&self) -> axum::http::StatusCode {
        match self {
            OAuthRouteError::BadRequest(_) => axum::http::StatusCode::BAD_REQUEST,
            OAuthRouteError::ConversationNotFound => axum::http::StatusCode::NOT_FOUND,
            OAuthRouteError::NotConfigured => axum::http::StatusCode::NOT_FOUND,
            OAuthRouteError::Url(_)
            | OAuthRouteError::Storage(_)
            | OAuthRouteError::Exchange(_)
            | OAuthRouteError::Resolve(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            OAuthRouteError::Resume(error) => error.status(),
        }
    }
}

impl IntoResponse for OAuthRouteError {
    fn into_response(self) -> Response {
        match self {
            OAuthRouteError::Resume(err) | OAuthRouteError::Resolve(err) => err.into_response(),
            OAuthRouteError::BadRequest(message) => {
                (self.as_status(), Html(message)).into_response()
            }
            OAuthRouteError::ConversationNotFound | OAuthRouteError::NotConfigured => {
                (self.as_status(), Html("not found")).into_response()
            }
            OAuthRouteError::Url(_)
            | OAuthRouteError::Storage(_)
            | OAuthRouteError::Exchange(_) => {
                (self.as_status(), Html("internal error")).into_response()
            }
        }
    }
}

impl From<WebChatError> for OAuthRouteError {
    fn from(value: WebChatError) -> Self {
        OAuthRouteError::Resume(value)
    }
}

pub const CLOSE_WINDOW_HTML: &str =
    "<!DOCTYPE html><html><body>You can close this window.</body></html>";

#[async_trait]
pub trait GreenticOauthClient: Send + Sync {
    async fn exchange_code(
        &self,
        tenant_ctx: &TenantCtx,
        config: &OAuthProviderConfig,
        code: &str,
        redirect_uri: &str,
    ) -> Result<String, anyhow::Error>;
}

pub struct ReqwestGreenticOauthClient {
    client: Client,
}

impl ReqwestGreenticOauthClient {
    pub fn new(client: Client) -> Self {
        Self { client }
    }
}

#[derive(Debug, Deserialize)]
struct TokenExchangeResponse {
    token_handle: String,
}

#[async_trait]
impl GreenticOauthClient for ReqwestGreenticOauthClient {
    async fn exchange_code(
        &self,
        _tenant_ctx: &TenantCtx,
        config: &OAuthProviderConfig,
        code: &str,
        redirect_uri: &str,
    ) -> Result<String, anyhow::Error> {
        let token_url = format!("{}/token", config.issuer.trim_end_matches('/'));
        let response = self
            .client
            .post(token_url)
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("client_id", config.client_id.as_str()),
                ("redirect_uri", redirect_uri),
            ])
            .send()
            .await
            .context("oauth token request failed")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<unreadable>".to_string());
            return Err(anyhow!("oauth exchange failed ({status}): {body}"));
        }

        let body = response
            .json::<TokenExchangeResponse>()
            .await
            .context("oauth token decode failed")?;

        if body.token_handle.trim().is_empty() {
            return Err(anyhow!("oauth token handle missing in response"));
        }

        Ok(body.token_handle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_oauth_card() {
        let activity = json!({
            "type": "message",
            "attachments": [
                {"contentType": "application/vnd.microsoft.card.oauth"}
            ]
        });
        assert!(contains_oauth_card(&activity));
    }

    #[test]
    fn ignores_non_oauth_card() {
        let activity = json!({
            "type": "message",
            "attachments": [
                {"contentType": "application/vnd.microsoft.card.adaptive"}
            ]
        });
        assert!(!contains_oauth_card(&activity));
    }
}
