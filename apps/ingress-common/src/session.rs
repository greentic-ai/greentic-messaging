use anyhow::Result;
use gsm_core::{InvocationEnvelope, MessageEnvelope, TenantCtx};
pub use gsm_session::SharedSessionStore;
use gsm_session::{ConversationScope, store_from_env};
use tracing::warn;

/// Constructs a shared session store using environment configuration.
pub async fn init_session_store() -> Result<SharedSessionStore> {
    store_from_env().await
}

/// Attaches the active session identifier (if any) to the invocation context.
pub async fn attach_session_id(
    store: &SharedSessionStore,
    ctx: &TenantCtx,
    env: &MessageEnvelope,
    invocation: &mut InvocationEnvelope,
) {
    if invocation.ctx.session_id.is_some() {
        return;
    }

    let scope = ConversationScope::new(
        ctx.env.as_str(),
        ctx.tenant.as_str(),
        env.platform.as_str(),
        &env.chat_id,
        &env.user_id,
        env.thread_id.clone(),
    );

    match store.find_by_scope(&scope).await {
        Ok(Some(record)) => {
            invocation.ctx.session_id = Some(record.key.as_str().to_string());
        }
        Ok(None) => {}
        Err(err) => {
            warn!(error = %err, "session lookup failed; proceeding without session");
        }
    }
}
