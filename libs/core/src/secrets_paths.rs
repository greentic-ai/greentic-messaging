use crate::prelude::*;

pub fn messaging_credentials(platform: &str, ctx: &TenantCtx) -> SecretPath {
    let team = ctx.team.as_ref().map(|t| t.0.as_str()).unwrap_or("default");
    SecretPath(format!(
        "/{}/messaging/{}/{}/{}/credentials.json",
        ctx.env.0, platform, ctx.tenant.0, team
    ))
}
