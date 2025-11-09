use std::env;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OauthStartRequest {
    pub provider: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scopes: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tenant: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub team: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relay: Option<OauthRelayContext>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OauthRelayContext {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_message_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
}

#[derive(Debug, Clone)]
pub struct OauthClient<T: StartTransport = ReqwestTransport> {
    transport: T,
    base_url: Url,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartLink {
    pub url: Url,
    pub connection_name: Option<String>,
}

impl<T: StartTransport> OauthClient<T> {
    pub fn with_transport(transport: T, base_url: Url) -> Self {
        Self {
            transport,
            base_url,
        }
    }

    pub async fn build_start_url(&self, request: &OauthStartRequest) -> Result<StartLink> {
        let endpoint = self
            .base_url
            .join("oauth/start")
            .context("failed to resolve /oauth/start")?;
        let response = self.transport.post_start(endpoint, request).await?;
        let url = Url::parse(&response.url).context("oauth/start returned invalid URL")?;
        let connection_name = response
            .connection_name
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.to_string());
        Ok(StartLink {
            url,
            connection_name,
        })
    }
}

impl OauthClient<ReqwestTransport> {
    pub fn new(http: Client, base_url: Url) -> Self {
        Self::with_transport(ReqwestTransport::new(http), base_url)
    }

    pub fn from_env(http: Client) -> Result<Self> {
        let raw = env::var("OAUTH_BASE_URL").context("OAUTH_BASE_URL must be set")?;
        let base_url = Url::parse(&raw).context("invalid OAUTH_BASE_URL")?;
        Ok(Self::new(http, base_url))
    }
}

#[async_trait]
pub trait StartTransport: Send + Sync {
    async fn post_start(&self, url: Url, payload: &OauthStartRequest) -> Result<StartResponse>;
}

#[derive(Clone)]
pub struct ReqwestTransport {
    http: Client,
}

impl ReqwestTransport {
    pub fn new(http: Client) -> Self {
        Self { http }
    }
}

#[async_trait]
impl StartTransport for ReqwestTransport {
    async fn post_start(&self, url: Url, payload: &OauthStartRequest) -> Result<StartResponse> {
        let response = self
            .http
            .post(url)
            .json(payload)
            .send()
            .await
            .context("failed to call oauth/start")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<unavailable>".into());
            bail!("oauth/start returned {status}: {body}");
        }

        let payload = response
            .json::<StartResponse>()
            .await
            .context("invalid oauth/start response body")?;
        Ok(payload)
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct StartResponse {
    url: String,
    #[serde(default)]
    connection_name: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    #[tokio::test]
    async fn client_posts_payload_and_returns_url() {
        let transport = MockTransport::new(
            "https://oauth.example/oauth/start",
            Ok(StartResponse {
                url: "https://oauth.example/start/abc123".into(),
                connection_name: Some("m365".into()),
            }),
        );
        let client = OauthClient::with_transport(
            transport.clone(),
            Url::parse("https://oauth.example/").unwrap(),
        );

        let request = OauthStartRequest {
            provider: "microsoft".into(),
            scopes: vec!["User.Read".into()],
            resource: Some("https://graph.microsoft.com".into()),
            prompt: Some("consent".into()),
            tenant: Some("acme".into()),
            team: Some("support".into()),
            user: Some("user-1".into()),
            relay: Some(OauthRelayContext {
                provider_message_id: Some("abc123".into()),
                platform: Some("teams".into()),
            }),
            metadata: Some(json!({"variant":"beta"})),
        };

        let link = client.build_start_url(&request).await.expect("start url");
        assert_eq!(link.url.as_str(), "https://oauth.example/start/abc123");
        assert_eq!(link.connection_name.as_deref(), Some("m365"));
        assert_eq!(transport.captured_count(), 1);
    }

    #[tokio::test]
    async fn client_surface_errors_are_returned() {
        let transport = MockTransport::new(
            "https://oauth.example/oauth/start",
            Err("missing provider".into()),
        );
        let client = OauthClient::with_transport(
            transport.clone(),
            Url::parse("https://oauth.example/").unwrap(),
        );
        let request = OauthStartRequest {
            provider: "microsoft".into(),
            scopes: Vec::new(),
            resource: None,
            prompt: None,
            tenant: None,
            team: None,
            user: None,
            relay: None,
            metadata: None,
        };

        let err = client.build_start_url(&request).await.unwrap_err();
        assert!(
            err.to_string().contains("missing provider"),
            "unexpected error: {err}"
        );
        assert_eq!(transport.captured_count(), 1);
    }

    #[derive(Clone)]
    struct MockTransport {
        expected_url: String,
        response: Arc<Mutex<Result<StartResponse, String>>>,
        captured: Arc<Mutex<Vec<OauthStartRequest>>>,
    }

    impl MockTransport {
        fn new(expected_url: &str, response: Result<StartResponse, String>) -> Self {
            Self {
                expected_url: expected_url.into(),
                response: Arc::new(Mutex::new(response)),
                captured: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn captured_count(&self) -> usize {
            self.captured.lock().unwrap().len()
        }
    }

    #[async_trait]
    impl StartTransport for MockTransport {
        async fn post_start(&self, url: Url, payload: &OauthStartRequest) -> Result<StartResponse> {
            assert_eq!(url.as_str(), self.expected_url);
            self.captured.lock().unwrap().push(payload.clone());
            let outcome = self.response.lock().unwrap().clone();
            outcome.map_err(|err| anyhow!(err))
        }
    }
}
