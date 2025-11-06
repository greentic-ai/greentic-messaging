use crate::{http::RawRequest, prelude::*};
use serde_json::Value;

#[async_trait::async_trait]
pub trait Ingress: Send + Sync {
    async fn to_envelope(&self, req: &RawRequest) -> NodeResult<InvocationEnvelope>;
}

#[derive(Debug, Clone)]
pub struct VerifiedEvent {
    pub tenant_ctx: TenantCtx,
    pub payload: Value,
}
