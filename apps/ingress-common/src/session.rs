use anyhow::Result;
use greentic_types::UserId;
pub use gsm_core::session::SharedSessionStore;
use gsm_core::session::store_from_env;
use gsm_core::{InvocationEnvelope, MessageEnvelope, TenantCtx};
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

    let Some(user_id) = user_id(ctx, env) else {
        return;
    };
    match store.find_by_user(ctx.clone(), user_id).await {
        Ok(Some((session_key, _))) => {
            invocation.ctx.session_id = Some(session_key.as_str().to_string());
        }
        Ok(None) => {}
        Err(err) => warn!(error = %err, "session lookup failed; proceeding without session"),
    }
}

fn user_id(ctx: &TenantCtx, env: &MessageEnvelope) -> Option<UserId> {
    ctx.user_id
        .clone()
        .or_else(|| ctx.user.clone())
        .or_else(|| UserId::try_from(env.user_id.as_str()).ok())
}
