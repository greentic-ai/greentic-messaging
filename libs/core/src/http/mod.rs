use anyhow::Result;
use async_trait::async_trait;
use bytes::Bytes;
use http::{HeaderMap, Request, StatusCode};

#[derive(Debug, Clone)]
pub struct RawResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Bytes,
}

pub type RawRequest = Request<Bytes>;

#[async_trait]
pub trait HttpClient: Send + Sync {
    async fn execute(&self, request: Request<Bytes>) -> Result<RawResponse>;
}
