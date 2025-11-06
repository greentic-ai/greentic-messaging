use std::time::{Duration, Instant};

use async_trait::async_trait;
use http::StatusCode;
use metrics::{counter, histogram};
use reqwest::{Client, Url};
use serde::Deserialize;
use thiserror::Error;
use tokio::sync::Mutex;

#[derive(Debug, Clone)]
pub struct TokenResponse {
    pub token: String,
    pub expires_in: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct ConversationResponse {
    pub token: String,
    pub conversation_id: String,
    pub stream_url: Option<String>,
    pub expires_in: Option<u64>,
}

#[async_trait]
pub trait DirectLineApi: Send + Sync {
    async fn generate_token(&self, secret: &str) -> Result<TokenResponse, DirectLineError>;
    async fn start_conversation(
        &self,
        token: &str,
    ) -> Result<ConversationResponse, DirectLineError>;
}

pub struct ReqwestDirectLineApi {
    client: Client,
    base_url: Url,
}

impl ReqwestDirectLineApi {
    pub fn new(client: Client, base_url: &str) -> Result<Self, DirectLineError> {
        let mut url = Url::parse(base_url).map_err(|err| DirectLineError::Config(err.into()))?;
        if !base_url.ends_with('/') {
            url = url
                .join("./")
                .map_err(|err| DirectLineError::Config(err.into()))?;
        }
        Ok(Self {
            client,
            base_url: url,
        })
    }

    fn endpoint(&self, path: &str) -> Result<Url, DirectLineError> {
        self.base_url
            .join(path)
            .map_err(|err| DirectLineError::Config(err.into()))
    }
}

#[async_trait]
impl DirectLineApi for ReqwestDirectLineApi {
    async fn generate_token(&self, secret: &str) -> Result<TokenResponse, DirectLineError> {
        let url = self.endpoint("tokens/generate")?;
        let started = Instant::now();
        let response = self
            .client
            .post(url)
            .bearer_auth(secret)
            .json(&serde_json::json!({}))
            .send()
            .await
            .map_err(|err| {
                counter!(
                    "webchat_errors_total",
                    "kind" => "directline_transport",
                    "endpoint" => "tokens.generate"
                )
                .increment(1);
                DirectLineError::Transport(err)
            })?;

        let status = response.status();
        let status_label = status.as_str().to_string();
        histogram!(
            "webchat_dl_roundtrip_seconds",
            "endpoint" => "tokens.generate",
            "status" => status_label
        )
        .record(started.elapsed().as_secs_f64());

        map_response("tokens.generate", response)
            .await
            .map(|raw: RawTokenResponse| TokenResponse {
                token: raw.token,
                expires_in: raw.expires_in,
            })
    }

    async fn start_conversation(
        &self,
        token: &str,
    ) -> Result<ConversationResponse, DirectLineError> {
        let url = self.endpoint("conversations")?;
        let started = Instant::now();
        let response = self
            .client
            .post(url)
            .bearer_auth(token)
            .json(&serde_json::json!({}))
            .send()
            .await
            .map_err(|err| {
                counter!(
                    "webchat_errors_total",
                    "kind" => "directline_transport",
                    "endpoint" => "conversations.start"
                )
                .increment(1);
                DirectLineError::Transport(err)
            })?;

        let status = response.status();
        let status_label = status.as_str().to_string();
        histogram!(
            "webchat_dl_roundtrip_seconds",
            "endpoint" => "conversations.start",
            "status" => status_label
        )
        .record(started.elapsed().as_secs_f64());

        map_response("conversations.start", response)
            .await
            .map(|raw: RawConversationResponse| ConversationResponse {
                token: raw.token,
                conversation_id: raw.conversation_id,
                stream_url: raw.stream_url,
                expires_in: raw.expires_in,
            })
    }
}

async fn map_response<T>(
    endpoint: &'static str,
    response: reqwest::Response,
) -> Result<T, DirectLineError>
where
    T: for<'de> Deserialize<'de>,
{
    let status = response.status();
    if !status.is_success() {
        let status_label = status.as_str().to_string();
        let retry_after = retry_after(&response);
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "<unreadable>".into());
        counter!(
            "webchat_errors_total",
            "kind" => "directline_remote",
            "endpoint" => endpoint,
            "status" => status_label
        )
        .increment(1);
        return Err(DirectLineError::Remote {
            status,
            retry_after,
            message: if body.len() > 512 {
                body[..512].to_string()
            } else {
                body
            },
        });
    }

    let body = response.json::<T>().await.map_err(|err| {
        counter!(
            "webchat_errors_total",
            "kind" => "directline_decode",
            "endpoint" => endpoint
        )
        .increment(1);
        DirectLineError::Decode(err.into())
    })?;
    Ok(body)
}

fn retry_after(response: &reqwest::Response) -> Option<Duration> {
    response
        .headers()
        .get("retry-after")
        .and_then(|header| header.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_secs)
}

#[derive(Debug, Error)]
pub enum DirectLineError {
    #[error("direct line configuration error")]
    Config(anyhow::Error),
    #[error("direct line transport error")]
    Transport(#[source] reqwest::Error),
    #[error("direct line remote error (status {status}, retry_after = {retry_after:?})")]
    Remote {
        status: StatusCode,
        retry_after: Option<Duration>,
        message: String,
    },
    #[error("direct line response decode error")]
    Decode(anyhow::Error),
}

#[derive(Debug, Deserialize)]
struct RawTokenResponse {
    token: String,
    #[serde(default)]
    expires_in: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct RawConversationResponse {
    token: String,
    #[serde(rename = "conversationId")]
    conversation_id: String,
    #[serde(rename = "streamUrl")]
    #[serde(default)]
    stream_url: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
}

pub struct MockDirectLineApi {
    pub generated: Mutex<Vec<String>>,
    pub conversations: Mutex<Vec<String>>,
    pub token: TokenResponse,
    pub conversation: ConversationResponse,
}

impl MockDirectLineApi {
    pub fn new(token: TokenResponse, conversation: ConversationResponse) -> Self {
        Self {
            generated: Mutex::new(Vec::new()),
            conversations: Mutex::new(Vec::new()),
            token,
            conversation,
        }
    }
}

impl Default for MockDirectLineApi {
    fn default() -> Self {
        Self::new(
            TokenResponse {
                token: String::new(),
                expires_in: None,
            },
            ConversationResponse {
                token: String::new(),
                conversation_id: String::new(),
                stream_url: None,
                expires_in: None,
            },
        )
    }
}

#[async_trait]
impl DirectLineApi for MockDirectLineApi {
    async fn generate_token(&self, secret: &str) -> Result<TokenResponse, DirectLineError> {
        self.generated.lock().await.push(secret.to_string());
        Ok(self.token.clone())
    }

    async fn start_conversation(
        &self,
        token: &str,
    ) -> Result<ConversationResponse, DirectLineError> {
        self.conversations.lock().await.push(token.to_string());
        Ok(self.conversation.clone())
    }
}
