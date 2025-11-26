//! Helpers for converting core types to the canonical host bindings.
use greentic_interfaces_host::bindings;
use greentic_types::{GreenticError, SessionCursor, TenantCtx};
use std::convert::TryFrom;

pub type HostTenantCtx = bindings::greentic::interfaces_types::types::TenantCtx;
pub type HostSessionCursor = bindings::greentic::interfaces_types::types::SessionCursor;

// Surface common host-facing modules so callers can import everything via gsm-core.
pub use greentic_interfaces_host::{
    http, messaging, messaging_session, oauth, oauth_broker, secrets, state, telemetry,
};

/// Map a `TenantCtx` into its WIT host binding representation.
pub fn to_host_tenant_ctx(ctx: &TenantCtx) -> Result<HostTenantCtx, GreenticError> {
    HostTenantCtx::try_from(ctx.clone())
}

/// Map a WIT host binding tenant context back into the core type.
pub fn from_host_tenant_ctx(wit: HostTenantCtx) -> Result<TenantCtx, GreenticError> {
    TenantCtx::try_from(wit)
}

/// Map a session cursor into its host binding counterpart.
pub fn to_host_session_cursor(cursor: SessionCursor) -> HostSessionCursor {
    cursor.into()
}

/// Map a host binding session cursor back into the core type.
pub fn from_host_session_cursor(cursor: HostSessionCursor) -> SessionCursor {
    cursor.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use greentic_types::{GreenticError, SessionCursor, TenantCtx};

    fn fixture_id<T>(value: &str) -> T
    where
        T: TryFrom<String, Error = GreenticError>,
    {
        T::try_from(value.to_owned())
            .unwrap_or_else(|err| panic!("invalid fixture identifier '{value}': {err}"))
    }

    fn sample_tenant_ctx() -> TenantCtx {
        TenantCtx {
            env: fixture_id("prod"),
            tenant: fixture_id("tenant-1"),
            tenant_id: fixture_id("tenant-1"),
            team: Some(fixture_id("team-42")),
            team_id: Some(fixture_id("team-42")),
            user: Some(fixture_id("user-7")),
            user_id: Some(fixture_id("user-7")),
            session_id: Some("sess-42".into()),
            flow_id: Some("flow-42".into()),
            node_id: Some("node-42".into()),
            provider_id: Some("provider-42".into()),
            trace_id: Some("trace".into()),
            correlation_id: Some("corr".into()),
            deadline: None,
            attempt: 2,
            idempotency_key: Some("idem".into()),
            impersonation: None,
            attributes: Default::default(),
        }
    }

    #[test]
    fn tenant_ctx_roundtrips_via_host_bindings() {
        let ctx = sample_tenant_ctx();
        let wit = to_host_tenant_ctx(&ctx).expect("map to host");
        let back = from_host_tenant_ctx(wit).expect("map from host");
        assert_eq!(back.env, ctx.env);
        assert_eq!(back.tenant, ctx.tenant);
        assert_eq!(back.user, ctx.user);
        assert_eq!(back.session_id, ctx.session_id);
        assert_eq!(back.flow_id, ctx.flow_id);
        assert_eq!(back.provider_id, ctx.provider_id);
    }

    #[test]
    fn session_cursor_roundtrips_via_host_bindings() {
        let cursor = SessionCursor {
            node_pointer: "node".into(),
            wait_reason: Some("waiting".into()),
            outbox_marker: Some("marker".into()),
        };
        let wit = to_host_session_cursor(cursor.clone());
        let back = from_host_session_cursor(wit);
        assert_eq!(back.node_pointer, cursor.node_pointer);
        assert_eq!(back.wait_reason, cursor.wait_reason);
        assert_eq!(back.outbox_marker, cursor.outbox_marker);
    }
}
