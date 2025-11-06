use std::{collections::HashMap, sync::Arc};

use anyhow::Result;
use async_trait::async_trait;
use greentic_types::TenantCtx;
use time::OffsetDateTime;
use tokio::sync::RwLock;

#[derive(Clone, Debug)]
pub struct WebchatSession {
    pub conversation_id: String,
    pub tenant_ctx: TenantCtx,
    pub bearer_token: String,
    pub watermark: Option<String>,
    pub last_seen_at: OffsetDateTime,
    pub proactive_ok: bool,
}

impl WebchatSession {
    pub fn new(conversation_id: String, tenant_ctx: TenantCtx, bearer_token: String) -> Self {
        Self {
            conversation_id,
            tenant_ctx,
            bearer_token,
            watermark: None,
            last_seen_at: OffsetDateTime::now_utc(),
            proactive_ok: true,
        }
    }
}

#[async_trait]
pub trait WebchatSessionStore: Send + Sync {
    async fn get(&self, conversation_id: &str) -> Result<Option<WebchatSession>>;
    async fn upsert(&self, session: WebchatSession) -> Result<()>;
    async fn update_watermark(
        &self,
        conversation_id: &str,
        watermark: Option<String>,
    ) -> Result<()>;
    async fn update_bearer_token(&self, conversation_id: &str, token: String) -> Result<()>;
    async fn set_proactive(&self, conversation_id: &str, proactive_ok: bool) -> Result<()>;
    async fn list_by_tenant(
        &self,
        env: &str,
        tenant: &str,
        team: Option<&str>,
    ) -> Result<Vec<WebchatSession>>;
}

#[derive(Default)]
pub struct MemorySessionStore {
    inner: RwLock<HashMap<String, WebchatSession>>,
}

impl MemorySessionStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl WebchatSessionStore for MemorySessionStore {
    async fn get(&self, conversation_id: &str) -> Result<Option<WebchatSession>> {
        let guard = self.inner.read().await;
        Ok(guard.get(conversation_id).cloned())
    }

    async fn upsert(&self, session: WebchatSession) -> Result<()> {
        let mut guard = self.inner.write().await;
        guard.insert(session.conversation_id.clone(), session);
        Ok(())
    }

    async fn update_watermark(
        &self,
        conversation_id: &str,
        watermark: Option<String>,
    ) -> Result<()> {
        let mut guard = self.inner.write().await;
        if let Some(existing) = guard.get_mut(conversation_id) {
            existing.watermark = watermark;
            existing.last_seen_at = OffsetDateTime::now_utc();
        }
        Ok(())
    }

    async fn update_bearer_token(&self, conversation_id: &str, token: String) -> Result<()> {
        let mut guard = self.inner.write().await;
        if let Some(existing) = guard.get_mut(conversation_id) {
            existing.bearer_token = token;
            existing.last_seen_at = OffsetDateTime::now_utc();
        }
        Ok(())
    }

    async fn set_proactive(&self, conversation_id: &str, proactive_ok: bool) -> Result<()> {
        let mut guard = self.inner.write().await;
        if let Some(existing) = guard.get_mut(conversation_id) {
            existing.proactive_ok = proactive_ok;
            existing.last_seen_at = OffsetDateTime::now_utc();
        }
        Ok(())
    }

    async fn list_by_tenant(
        &self,
        env: &str,
        tenant: &str,
        team: Option<&str>,
    ) -> Result<Vec<WebchatSession>> {
        let env_lower = env.to_ascii_lowercase();
        let tenant_lower = tenant.to_ascii_lowercase();
        let team_lower = team.map(|value| value.to_ascii_lowercase());

        let guard = self.inner.read().await;
        Ok(guard
            .values()
            .filter(|session| {
                session
                    .tenant_ctx
                    .env
                    .as_ref()
                    .eq_ignore_ascii_case(&env_lower)
                    && session
                        .tenant_ctx
                        .tenant
                        .as_ref()
                        .eq_ignore_ascii_case(&tenant_lower)
                    && match (&team_lower, session.tenant_ctx.team.as_ref()) {
                        (Some(expected), Some(actual)) => {
                            actual.as_ref().eq_ignore_ascii_case(expected)
                        }
                        (Some(_), None) => false,
                        (None, _) => true,
                    }
            })
            .cloned()
            .collect())
    }
}

pub type SharedSessionStore = Arc<dyn WebchatSessionStore>;
