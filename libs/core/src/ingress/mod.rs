use crate::prelude::*;
use bytes::Bytes;
use http::Request;

#[async_trait::async_trait]
pub trait Ingress: Send + Sync {
    async fn to_envelope(&self, req: &Request<Bytes>) -> NodeResult<InvocationEnvelope>;
}
