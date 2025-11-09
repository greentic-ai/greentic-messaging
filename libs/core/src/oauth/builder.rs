use greentic_types::TenantCtx;
use serde_json::Value;

use crate::messaging_card::types::{OauthPrompt, OauthProvider};

use super::oauth_client::{OauthRelayContext, OauthStartRequest};

pub fn make_start_request(
    provider: &OauthProvider,
    scopes: &[String],
    resource: Option<&str>,
    prompt: Option<&OauthPrompt>,
    ctx: &TenantCtx,
    relay: Option<OauthRelayContext>,
    metadata: Option<&Value>,
) -> OauthStartRequest {
    OauthStartRequest {
        provider: provider.as_str().to_string(),
        scopes: scopes.to_vec(),
        resource: resource.map(|s| s.to_string()),
        prompt: prompt.map(|p| p.as_str().to_string()),
        tenant: Some(ctx.tenant.as_ref().to_string()),
        team: ctx.team.as_ref().map(|team| team.as_ref().to_string()),
        user: ctx.user.as_ref().map(|user| user.as_ref().to_string()),
        relay,
        metadata: metadata.cloned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use greentic_types::{EnvId, TeamId, TenantCtx, TenantId, UserId};

    fn tenant_ctx() -> TenantCtx {
        let mut ctx = TenantCtx::new(EnvId("dev".into()), TenantId("acme".into()));
        ctx.team = Some(TeamId("support".into()));
        ctx.user = Some(UserId("user-123".into()));
        ctx
    }

    #[test]
    fn make_start_request_populates_context() {
        let ctx = tenant_ctx();
        let request = make_start_request(
            &OauthProvider::Microsoft,
            &[String::from("User.Read")],
            Some("https://graph.microsoft.com"),
            Some(&OauthPrompt::Consent),
            &ctx,
            Some(OauthRelayContext {
                provider_message_id: Some("abc123".into()),
                platform: Some("teams".into()),
            }),
            Some(&Value::String("meta".into())),
        );

        assert_eq!(request.provider, "microsoft");
        assert_eq!(request.scopes, vec!["User.Read"]);
        assert_eq!(
            request.resource.as_deref(),
            Some("https://graph.microsoft.com")
        );
        assert_eq!(request.prompt.as_deref(), Some("consent"));
        assert_eq!(request.tenant.as_deref(), Some("acme"));
        assert_eq!(request.team.as_deref(), Some("support"));
        assert_eq!(request.user.as_deref(), Some("user-123"));
        assert!(request.metadata.is_some());
        let relay = request.relay.expect("relay context");
        assert_eq!(relay.provider_message_id.as_deref(), Some("abc123"));
    }
}
