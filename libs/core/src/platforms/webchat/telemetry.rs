use greentic_types::TenantCtx;
use tracing::info_span;

use crate::platforms::webchat::provider::RouteContext;

pub fn span_for(action: &'static str, ctx: &RouteContext) -> tracing::Span {
    info_span!(
        "webchat.directline",
        action,
        env = ctx.env(),
        tenant = ctx.tenant(),
        team = ctx.team().unwrap_or("-")
    )
}

pub fn span_for_conversation(
    action: &'static str,
    ctx: &TenantCtx,
    conversation_id: &str,
) -> tracing::Span {
    info_span!(
        "webchat.conversation",
        action,
        env = ctx.env.as_ref(),
        tenant = ctx.tenant.as_ref(),
        team = ctx.team.as_ref().map(|t| t.as_ref()).unwrap_or("-"),
        conversation_id
    )
}

pub fn span_for_activity(
    action: &'static str,
    ctx: &TenantCtx,
    conversation_id: &str,
    activity_id: &str,
) -> tracing::Span {
    info_span!(
        "webchat.activity",
        action,
        env = ctx.env.as_ref(),
        tenant = ctx.tenant.as_ref(),
        team = ctx.team.as_ref().map(|t| t.as_ref()).unwrap_or("-"),
        conversation_id,
        activity_id
    )
}

pub fn team_or_dash(team: Option<&str>) -> &str {
    team.filter(|s| !s.is_empty()).unwrap_or("-")
}

pub fn tenant_labels(ctx: &TenantCtx) -> (&str, &str, &str) {
    (
        ctx.env.as_ref(),
        ctx.tenant.as_ref(),
        ctx.team.as_ref().map(|t| t.as_ref()).unwrap_or("-"),
    )
}
