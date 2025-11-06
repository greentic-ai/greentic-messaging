use std::sync::Arc;

use crate::{
    cards::Card,
    http::RawRequest,
    ingress::VerifiedEvent,
    platforms::provider::{PlatformInit, PlatformProvider},
};
use anyhow::{Context as _, Result, anyhow, bail};
use async_trait::async_trait;
use greentic_secrets::spec::{Scope, SecretUri, SecretsBackend};
use greentic_types::TenantCtx;

use super::config::{Config, OAuthProviderConfig, SigningKeys};

/// WebChat provider wiring with secrets backend integration.
#[derive(Clone)]
pub struct WebChatProvider {
    config: Config,
    secrets: Arc<dyn SecretsBackend + Send + Sync + 'static>,
    signing_scope: Option<Scope>,
    #[allow(dead_code)]
    init: Option<PlatformInit>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouteContext {
    env: String,
    tenant: String,
    team: Option<String>,
}

impl RouteContext {
    pub fn new(env: String, tenant: String, team: Option<String>) -> Self {
        Self { env, tenant, team }
    }

    pub fn env(&self) -> &str {
        &self.env
    }
    pub fn tenant(&self) -> &str {
        &self.tenant
    }

    pub fn team(&self) -> Option<&str> {
        self.team.as_deref()
    }
}

impl WebChatProvider {
    pub fn new(config: Config, secrets: Arc<dyn SecretsBackend + Send + Sync + 'static>) -> Self {
        Self {
            config,
            secrets,
            signing_scope: None,
            init: None,
        }
    }

    pub fn with_signing_scope(mut self, scope: Scope) -> Self {
        self.signing_scope = Some(scope);
        self
    }

    pub fn with_platform_init(mut self, init: PlatformInit) -> Self {
        self.init = Some(init);
        self
    }

    pub fn signing_scope(&self) -> Option<&Scope> {
        self.signing_scope.as_ref()
    }

    pub fn secrets(&self) -> Arc<dyn SecretsBackend + Send + Sync + 'static> {
        Arc::clone(&self.secrets)
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub async fn signing_keys(&self) -> anyhow::Result<SigningKeys> {
        let scope = self
            .signing_scope
            .clone()
            .ok_or_else(|| anyhow!("signing scope not configured"))?;
        let secret = self
            .fetch_secret(scope, WEBCHAT_CATEGORY, JWT_SIGNING_KEY_NAME)
            .await?
            .ok_or_else(|| anyhow!("missing webchat/jwt_signing_key secret"))?;
        Ok(SigningKeys { secret })
    }

    pub async fn direct_line_secret(&self, ctx: &TenantCtx) -> anyhow::Result<Option<String>> {
        self.scoped_secret_with_fallback(ctx, WEBCHAT_CATEGORY, CHANNEL_TOKEN_NAME)
            .await
    }

    pub async fn oauth_config(
        &self,
        ctx: &TenantCtx,
    ) -> anyhow::Result<Option<OAuthProviderConfig>> {
        let issuer = match self
            .scoped_secret_with_fallback(ctx, WEBCHAT_OAUTH_CATEGORY, OAUTH_ISSUER_NAME)
            .await?
        {
            Some(value) => value,
            None => return Ok(None),
        };
        let client_id = match self
            .scoped_secret_with_fallback(ctx, WEBCHAT_OAUTH_CATEGORY, OAUTH_CLIENT_ID_NAME)
            .await?
        {
            Some(value) => value,
            None => return Ok(None),
        };
        let redirect_base = match self
            .scoped_secret_with_fallback(ctx, WEBCHAT_OAUTH_CATEGORY, OAUTH_REDIRECT_BASE_NAME)
            .await?
        {
            Some(value) => value,
            None => return Ok(None),
        };

        Ok(Some(OAuthProviderConfig {
            issuer,
            client_id,
            redirect_base,
        }))
    }

    pub async fn oauth_client_secret(&self, ctx: &TenantCtx) -> anyhow::Result<Option<String>> {
        self.scoped_secret_with_fallback(ctx, WEBCHAT_OAUTH_CATEGORY, OAUTH_CLIENT_SECRET_NAME)
            .await
    }

    async fn scoped_secret_with_fallback(
        &self,
        ctx: &TenantCtx,
        category: &str,
        name: &str,
    ) -> anyhow::Result<Option<String>> {
        if let Some(team) = ctx.team.as_ref() {
            let scope = scope_from_ctx(ctx, Some(team.as_ref().to_string()))?;
            if let Some(value) = self.fetch_secret(scope, category, name).await? {
                return Ok(Some(value));
            }
        }
        let scope = scope_from_ctx(ctx, None)?;
        self.fetch_secret(scope, category, name).await
    }

    async fn fetch_secret(
        &self,
        scope: Scope,
        category: &str,
        name: &str,
    ) -> anyhow::Result<Option<String>> {
        let backend = Arc::clone(&self.secrets);
        let category = category.to_string();
        let name = name.to_string();
        tokio::task::spawn_blocking(move || {
            let uri = SecretUri::new(scope, category, name)?;
            let secret = backend.get(&uri, None).map_err(|err| anyhow!(err))?;
            if let Some(secret) = secret {
                if secret.deleted {
                    return Ok(None);
                }
                if let Some(record) = secret.record() {
                    let value = String::from_utf8(record.value.clone())
                        .context("secret value is not valid UTF-8")?
                        .trim()
                        .to_string();
                    if value.is_empty() {
                        Ok(None)
                    } else {
                        Ok(Some(value))
                    }
                } else {
                    Ok(None)
                }
            } else {
                Ok(None)
            }
        })
        .await
        .map_err(|err| anyhow!("failed to join secrets task: {err}"))?
    }
}

#[async_trait]
impl PlatformProvider for WebChatProvider {
    fn platform_id(&self) -> &'static str {
        "webchat"
    }

    async fn health(&self) -> Result<()> {
        Ok(())
    }

    async fn send_card(&self, _ctx: &TenantCtx, _to: &str, _card: &Card) -> Result<()> {
        bail!("WebChat provider not implemented yet")
    }

    async fn verify_webhook(&self, _raw: &RawRequest) -> Result<VerifiedEvent> {
        bail!("WebChat provider not implemented yet")
    }
}

const WEBCHAT_CATEGORY: &str = "webchat";
const JWT_SIGNING_KEY_NAME: &str = "jwt_signing_key";
const CHANNEL_TOKEN_NAME: &str = "channel_token";
const WEBCHAT_OAUTH_CATEGORY: &str = "webchat_oauth";
const OAUTH_ISSUER_NAME: &str = "issuer";
const OAUTH_CLIENT_ID_NAME: &str = "client_id";
const OAUTH_REDIRECT_BASE_NAME: &str = "redirect_base";
const OAUTH_CLIENT_SECRET_NAME: &str = "client_secret";

fn scope_from_ctx(ctx: &TenantCtx, team: Option<String>) -> anyhow::Result<Scope> {
    Scope::new(
        ctx.env.as_ref().to_ascii_lowercase(),
        ctx.tenant.as_ref().to_ascii_lowercase(),
        team.map(|value| value.to_ascii_lowercase()),
    )
    .map_err(|err| anyhow!(err))
}
