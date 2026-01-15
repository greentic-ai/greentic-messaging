use async_trait::async_trait;
use std::sync::Arc;

use crate::{
    cards::{Card, CardRenderer},
    http::{HttpClient, RawRequest, RawResponse},
    ingress::VerifiedEvent,
    telemetry::TelemetryHandle,
};
use greentic_types::TenantCtx;
use secrets_core::SecretsBackend;

#[derive(Clone)]
pub struct PlatformInit {
    pub secrets: Arc<dyn SecretsBackend + Send + Sync>,
    pub telemetry: TelemetryHandle,
    pub http: Arc<dyn HttpClient + Send + Sync>,
    pub card_renderer: Arc<dyn CardRenderer>,
}

#[async_trait]
pub trait PlatformProvider: Send + Sync {
    fn platform_id(&self) -> &'static str;

    async fn health(&self) -> anyhow::Result<()>;

    async fn send_card(&self, ctx: &TenantCtx, to: &str, card: &Card) -> anyhow::Result<()>;

    async fn send_text(&self, ctx: &TenantCtx, to: &str, text: &str) -> anyhow::Result<()> {
        let card = Card::from_text(text);
        self.send_card(ctx, to, &card).await
    }

    async fn verify_webhook(&self, raw: &RawRequest) -> anyhow::Result<VerifiedEvent>;

    async fn raw_call(
        &self,
        _ctx: &TenantCtx,
        _method: &str,
        _path: &str,
        _body: Option<&[u8]>,
    ) -> anyhow::Result<RawResponse> {
        anyhow::bail!("raw_call not supported for {}", self.platform_id())
    }
}
