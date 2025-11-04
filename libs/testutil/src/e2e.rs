use crate::{TestConfig, secrets};
use anyhow::{Context, Result};
use gsm_core::egress::OutboundMessage;
use gsm_core::{TenantCtx, make_tenant_ctx};
use reqwest::Client;
use secrets_core::DefaultResolver;
use serde_json::Value;
use std::future::Future;
use std::sync::Arc;
use tokio::runtime::{Builder, Runtime};

/// Re-export common assertion helpers under the `e2e::assertions` namespace.
pub mod assertions {
    pub use crate::assertions::*;
}

/// Shared async test harness for end-to-end provider flows.
pub struct Harness {
    platform: String,
    ctx: TenantCtx,
    client: Client,
    runtime: Arc<Runtime>,
    config: Option<TestConfig>,
    resolver: Option<Arc<DefaultResolver>>,
}

impl Harness {
    /// Builds a new harness scoped to the provided platform identifier.
    pub fn new(platform: &str) -> Result<Self> {
        dotenvy::dotenv().ok();

        let config = TestConfig::from_env_or_secrets(platform)?;

        let runtime = Arc::new(
            Builder::new_multi_thread()
                .enable_all()
                .build()
                .context("failed to build tokio runtime for e2e harness")?,
        );

        let resolver = runtime
            .block_on(secrets::resolver())
            .context("failed to initialise secrets resolver")?;

        let tenant = config
            .as_ref()
            .and_then(|c| c.tenant.clone())
            .or_else(|| std::env::var("TENANT").ok())
            .unwrap_or_else(|| "acme".into());

        let team = config
            .as_ref()
            .and_then(|c| c.team.clone())
            .or_else(|| std::env::var("TEAM").ok());

        let user = std::env::var("USER").ok();

        let ctx = make_tenant_ctx(tenant.clone(), team.clone(), user);

        let client = Client::builder()
            .user_agent("gsm-e2e-tests")
            .build()
            .context("failed to construct reqwest client")?;

        Ok(Self {
            platform: platform.to_string(),
            ctx,
            client,
            runtime,
            config,
            resolver,
        })
    }

    /// Returns the tenant context configured for this harness.
    pub fn ctx(&self) -> &TenantCtx {
        &self.ctx
    }

    /// Clones the HTTP client configured for the harness.
    pub fn client(&self) -> Client {
        self.client.clone()
    }

    /// Returns the secrets resolver when available.
    pub fn resolver(&self) -> Option<Arc<DefaultResolver>> {
        self.resolver.clone()
    }

    /// Exposes the detected test configuration.
    pub fn config(&self) -> Option<&TestConfig> {
        self.config.as_ref()
    }

    /// Provides access to the underlying runtime for async operations.
    pub fn block_on<F>(&self, fut: F) -> F::Output
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.runtime.block_on(fut)
    }

    /// Creates a text-only outbound payload targeted to the given channel.
    pub fn outbound_text(
        &self,
        channel: impl Into<String>,
        text: impl Into<String>,
    ) -> OutboundMessage {
        OutboundMessage {
            channel: Some(channel.into()),
            text: Some(text.into()),
            payload: None,
        }
    }

    /// Creates an outbound payload carrying structured content.
    pub fn outbound_payload(&self, channel: impl Into<String>, payload: Value) -> OutboundMessage {
        OutboundMessage {
            channel: Some(channel.into()),
            text: None,
            payload: Some(payload),
        }
    }

    /// Returns the platform associated with this harness.
    pub fn platform(&self) -> &str {
        &self.platform
    }
}
