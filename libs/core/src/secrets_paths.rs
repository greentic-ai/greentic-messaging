use crate::prelude::*;

pub fn messaging_credentials(platform: &str, ctx: &TenantCtx) -> SecretPath {
    let team = ctx.team.as_ref().map(|t| t.0.as_str()).unwrap_or("default");
    SecretPath(format!(
        "/{}/messaging/{}/{}/{}/credentials.json",
        ctx.env.0, platform, ctx.tenant.0, team
    ))
}

pub fn slack_workspace_secret(ctx: &TenantCtx, workspace_id: &str) -> SecretPath {
    let team = ctx.team.as_ref().map(|t| t.0.as_str()).unwrap_or("default");
    SecretPath(format!(
        "/{}/messaging/slack/{}/{}/workspace/{}.json",
        ctx.env.0, ctx.tenant.0, team, workspace_id
    ))
}
