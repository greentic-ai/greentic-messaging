use crate::prelude::*;

fn scoped(ctx: &TenantCtx) -> SecretScope {
    SecretScope::new(
        ctx.env.0.clone(),
        ctx.tenant.0.clone(),
        ctx.team.as_ref().map(|t| t.0.clone()),
    )
    .expect("valid tenant scope")
}

pub fn messaging_credentials(platform: &str, ctx: &TenantCtx) -> SecretPath {
    let uri = SecretUri::new(
        scoped(ctx),
        "messaging",
        format!("{platform}.credentials.json"),
    )
    .expect("valid messaging credentials uri");
    SecretPath::new(uri)
}

pub fn slack_workspace_secret(ctx: &TenantCtx, workspace_id: &str) -> SecretPath {
    let workspace = workspace_id.to_lowercase();
    let uri = SecretUri::new(
        scoped(ctx),
        "messaging",
        format!("slack.workspace.{workspace}.json"),
    )
    .expect("valid slack workspace uri");
    SecretPath::new(uri)
}

pub fn slack_workspace_index(ctx: &TenantCtx) -> SecretPath {
    let uri = SecretUri::new(
        scoped(ctx),
        "messaging",
        "slack.workspace.index.json".to_string(),
    )
    .expect("valid slack workspace index uri");
    SecretPath::new(uri)
}

pub fn teams_conversations_secret(ctx: &TenantCtx) -> SecretPath {
    let uri = SecretUri::new(
        scoped(ctx),
        "messaging",
        "teams.conversations.json".to_string(),
    )
    .expect("valid teams conversations uri");
    SecretPath::new(uri)
}

pub fn webex_credentials(ctx: &TenantCtx) -> SecretPath {
    let uri = SecretUri::new(
        scoped(ctx),
        "messaging",
        "webex.credentials.json".to_string(),
    )
    .expect("valid webex credentials uri");
    SecretPath::new(uri)
}

pub fn whatsapp_credentials(ctx: &TenantCtx) -> SecretPath {
    let uri = SecretUri::new(
        scoped(ctx),
        "messaging",
        "whatsapp.credentials.json".to_string(),
    )
    .expect("valid whatsapp credentials uri");
    SecretPath::new(uri)
}
