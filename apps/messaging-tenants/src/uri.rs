use anyhow::Context;
use greentic_secrets::spec::{Scope, SecretUri};
use gsm_core::Platform;

pub fn build_credentials_uri(
    env: &str,
    tenant: &str,
    team: Option<&str>,
    platform: Platform,
) -> anyhow::Result<String> {
    let team_name = team.unwrap_or("default");
    let scope = Scope::new(env, tenant, Some(team_name.to_string()))
        .context("failed to build secret scope")?;
    let name = format!(
        "{platform}-{team}-credentials.json",
        platform = platform.as_str(),
        team = team_name
    );
    let uri = SecretUri::new(scope, "messaging", name)?;
    Ok(uri.to_string())
}

pub fn build_placeholder_uri(env: &str, tenant: &str, team: &str) -> anyhow::Result<String> {
    let scope =
        Scope::new(env, tenant, Some(team.to_string())).context("failed to build secret scope")?;
    let name = format!("tenant-placeholder-{team}.json");
    let uri = SecretUri::new(scope, "messaging", name)?;
    Ok(uri.to_string())
}

#[cfg(test)]
mod tests {
    use super::build_credentials_uri;
    use greentic_secrets::spec::SecretUri;
    use gsm_core::Platform;

    #[test]
    fn credentials_uri_includes_team() {
        let uri = build_credentials_uri("dev", "acme", Some("support"), Platform::Slack)
            .expect("uri built");
        let parsed: SecretUri = uri.parse().unwrap();
        assert_eq!(parsed.scope().env(), "dev");
        assert_eq!(parsed.scope().tenant(), "acme");
        assert_eq!(parsed.scope().team(), Some("support"));
        assert_eq!(parsed.category(), "messaging");
        assert!(parsed.name().contains("slack"));
    }
}
