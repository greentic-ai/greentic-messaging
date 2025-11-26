use anyhow::Result;
use greentic_types::UserId;
use gsm_core::{InvocationEnvelope, MessageEnvelope, TenantCtx};
pub use gsm_session::SharedSessionStore;
use gsm_session::store_from_env;
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

    let user = ctx
        .user
        .clone()
        .or_else(|| ctx.user_id.clone())
        .or_else(|| UserId::try_from(env.user_id.as_str()).ok());
    let Some(user) = user else {
        warn!("no user identifier available for session lookup");
        return;
    };

    match store.find_by_user(ctx, &user).await {
        Ok(Some((key, _data))) => {
            invocation.ctx.session_id = Some(key.as_str().to_string());
        }
        Ok(None) => {}
        Err(err) => {
            warn!(error = %err, "session lookup failed; proceeding without session");
        }
    }
}
