use gsm_core::{MessageEnvelope, TenantCtx};
use gsm_session::ConversationScope;

/// Builds a [`ConversationScope`] from the incoming envelope and tenant context.
pub fn scope_from(tenant_ctx: &TenantCtx, env: &MessageEnvelope) -> ConversationScope {
    ConversationScope::new(
        tenant_ctx.env.as_str(),
        tenant_ctx.tenant.as_str(),
        env.platform.as_str(),
        &env.chat_id,
        &env.user_id,
        env.thread_id.clone(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use gsm_core::{MessageEnvelope, Platform, make_tenant_ctx};

    #[test]
    fn scope_captures_envelope_fields() {
        let ctx = make_tenant_ctx("acme".into(), Some("team-1".into()), Some("user-9".into()));
        let env = MessageEnvelope {
            tenant: "acme".into(),
            platform: Platform::Slack,
            chat_id: "C1".into(),
            user_id: "U1".into(),
            thread_id: Some("T1".into()),
            msg_id: "msg-1".into(),
            text: Some("hello".into()),
            timestamp: "2024-01-01T00:00:00Z".into(),
            context: Default::default(),
        };
        let scope = scope_from(&ctx, &env);
        assert_eq!(scope.tenant, "acme");
        assert_eq!(scope.platform, "slack");
        assert_eq!(scope.chat_id, "C1");
        assert_eq!(scope.user_id, "U1");
        assert_eq!(scope.thread_id.as_deref(), Some("T1"));
    }
}
