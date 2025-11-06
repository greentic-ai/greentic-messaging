use std::{env, sync::Arc};

use async_trait::async_trait;

use crate::auth::RouteContext;
use greentic_types::TenantCtx;

const DEFAULT_DIRECT_LINE_BASE: &str = "https://directline.botframework.com/v3/directline";
const DIRECT_LINE_BASE_ENV: &str = "WEBCHAT_DIRECT_LINE_BASE_URL";
const LEGACY_DIRECT_LINE_BASE_ENV: &str = "DL_BASE_URL";
const _JWT_SIGNING_KEY_SECRET: &str = "webchat/jwt/signing_key";
const JWT_SIGNING_KEY_ENV: &str = "WEBCHAT_JWT_SIGNING_KEY";

#[derive(Clone)]
pub struct Config {
    direct_line_base: String,
    oauth_lookup: Option<Arc<dyn Fn(&str) -> Option<String> + Send + Sync>>,
    signing_keys: Arc<dyn SigningKeyProvider + Send + Sync>,
}

impl Config {
    pub fn from_env() -> Self {
        let direct_line_base = env::var(DIRECT_LINE_BASE_ENV)
            .or_else(|_| env::var(LEGACY_DIRECT_LINE_BASE_ENV))
            .unwrap_or_else(|_| DEFAULT_DIRECT_LINE_BASE.to_string());
        Self {
            direct_line_base,
            oauth_lookup: None,
            signing_keys: Arc::new(EnvSigningKey::default()),
        }
    }

    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        Self {
            direct_line_base: base_url.into(),
            oauth_lookup: None,
            signing_keys: Arc::new(EnvSigningKey::default()),
        }
    }

    pub fn direct_line_base(&self) -> &str {
        &self.direct_line_base
    }

    pub fn resolve_secret(&self, ctx: &RouteContext) -> Option<String> {
        self.resolve_secret_with(ctx, |key| env::var(key).ok())
    }

    pub fn resolve_secret_with<F>(&self, ctx: &RouteContext, mut lookup: F) -> Option<String>
    where
        F: FnMut(&str) -> Option<String>,
    {
        let env = slug(ctx.env());
        let tenant = slug(ctx.tenant());
        let team = ctx.team().map(slug);

        let candidates = match team.as_deref() {
            Some(team) => vec![
                format!("WEBCHAT_DIRECT_LINE_SECRET__{env}__{tenant}__{team}"),
                format!("DL_SECRET__{env}__{tenant}__{team}"),
            ],
            None => vec![
                format!("WEBCHAT_DIRECT_LINE_SECRET__{env}__{tenant}"),
                format!("DL_SECRET__{env}__{tenant}"),
            ],
        };

        for key in candidates {
            if let Some(secret) = lookup_trimmed(&mut lookup, &key) {
                return Some(secret);
            }
        }

        if team.is_some() {
            for key in [
                format!("WEBCHAT_DIRECT_LINE_SECRET__{env}__{tenant}"),
                format!("DL_SECRET__{env}__{tenant}"),
            ] {
                if let Some(secret) = lookup_trimmed(&mut lookup, &key) {
                    return Some(secret);
                }
            }
        }

        None
    }

    pub fn resolve_oauth_config(&self, ctx: &TenantCtx) -> Option<OAuthProviderConfig> {
        if let Some(lookup) = &self.oauth_lookup {
            self.resolve_oauth_with(ctx, |key| lookup(key))
        } else {
            self.resolve_oauth_with(ctx, |key| std::env::var(key).ok())
        }
    }

    pub fn resolve_oauth_with<F>(
        &self,
        ctx: &TenantCtx,
        mut lookup: F,
    ) -> Option<OAuthProviderConfig>
    where
        F: FnMut(&str) -> Option<String>,
    {
        let env = slug(ctx.env.as_ref());
        let tenant = slug(ctx.tenant.as_ref());
        let team = ctx.team.as_ref().map(|team| slug(team.as_ref()));

        let issuer = Self::lookup_oauth_value(
            &mut lookup,
            &["WEBCHAT_OAUTH_ISSUER", "OAUTH_ISSUER"],
            &env,
            &tenant,
            team.as_deref(),
        )?;
        let client_id = Self::lookup_oauth_value(
            &mut lookup,
            &["WEBCHAT_OAUTH_CLIENT_ID", "OAUTH_CLIENT_ID"],
            &env,
            &tenant,
            team.as_deref(),
        )?;
        let redirect_base = Self::lookup_oauth_value(
            &mut lookup,
            &["WEBCHAT_OAUTH_REDIRECT_BASE", "OAUTH_REDIRECT_BASE"],
            &env,
            &tenant,
            team.as_deref(),
        )?;

        Some(OAuthProviderConfig {
            issuer,
            client_id,
            redirect_base,
        })
    }

    pub fn with_oauth_lookup<F>(mut self, lookup: F) -> Self
    where
        F: Fn(&str) -> Option<String> + Send + Sync + 'static,
    {
        self.oauth_lookup = Some(Arc::new(lookup));
        self
    }

    pub fn with_signing_keys_provider<P>(mut self, provider: P) -> Self
    where
        P: SigningKeyProvider + Send + Sync + 'static,
    {
        self.signing_keys = Arc::new(provider);
        self
    }

    pub async fn signing_keys(&self) -> anyhow::Result<SigningKeys> {
        self.signing_keys.fetch().await
    }

    fn lookup_oauth_value<F>(
        lookup: &mut F,
        keys: &[&str],
        env_slug: &str,
        tenant: &str,
        team: Option<&str>,
    ) -> Option<String>
    where
        F: FnMut(&str) -> Option<String>,
    {
        for key in keys {
            if let Some(team) = team {
                let var = format!("{key}__{env}__{tenant}__{team}", env = env_slug);
                if let Some(value) = lookup_trimmed(lookup, &var) {
                    return Some(value);
                }
            }

            let var = format!("{key}__{env}__{tenant}", env = env_slug);
            if let Some(value) = lookup_trimmed(lookup, &var) {
                return Some(value);
            }
        }
        None
    }
}

fn lookup_trimmed<F>(lookup: &mut F, key: &str) -> Option<String>
where
    F: FnMut(&str) -> Option<String>,
{
    lookup(key)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn slug(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

#[derive(Debug, Clone)]
pub struct SigningKeys {
    pub secret: String,
}

#[async_trait]
pub trait SigningKeyProvider {
    async fn fetch(&self) -> anyhow::Result<SigningKeys>;
}

#[derive(Default, Clone)]
struct EnvSigningKey;

#[async_trait]
impl SigningKeyProvider for EnvSigningKey {
    async fn fetch(&self) -> anyhow::Result<SigningKeys> {
        if let Ok(secret) = std::env::var(JWT_SIGNING_KEY_ENV) {
            return Ok(SigningKeys { secret });
        }
        // Placeholder: secret manager integration forthcoming.
        Err(anyhow::anyhow!(
            "JWT signing key missing (env + secrets unsupported yet)"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::HashMap, sync::Arc};

    #[test]
    fn resolves_team_specific_secret() {
        let ctx = RouteContext::new("dev".into(), "acme".into(), Some("support".into()));
        let mut map = HashMap::new();
        map.insert(
            "WEBCHAT_DIRECT_LINE_SECRET__DEV__ACME__SUPPORT".to_string(),
            "secret-a".to_string(),
        );
        let config = Config::with_base_url(DEFAULT_DIRECT_LINE_BASE);
        let secret = config.resolve_secret_with(&ctx, |key| map.get(key).cloned());
        assert_eq!(secret, Some("secret-a".into()));
    }

    #[test]
    fn falls_back_to_tenant_secret() {
        let ctx = RouteContext::new("dev".into(), "acme".into(), Some("support".into()));
        let mut map = HashMap::new();
        map.insert(
            "WEBCHAT_DIRECT_LINE_SECRET__DEV__ACME".to_string(),
            "secret-b".to_string(),
        );
        let config = Config::with_base_url(DEFAULT_DIRECT_LINE_BASE);
        let secret = config.resolve_secret_with(&ctx, |key| map.get(key).cloned());
        assert_eq!(secret, Some("secret-b".into()));
    }

    #[test]
    fn supports_legacy_direct_line_keys() {
        let ctx = RouteContext::new("dev".into(), "acme".into(), Some("support".into()));
        let mut map = HashMap::new();
        map.insert(
            "DL_SECRET__DEV__ACME__SUPPORT".to_string(),
            "legacy-secret".to_string(),
        );
        let config = Config::with_base_url(DEFAULT_DIRECT_LINE_BASE);
        let secret = config.resolve_secret_with(&ctx, |key| map.get(key).cloned());
        assert_eq!(secret, Some("legacy-secret".into()));
    }

    #[test]
    fn returns_none_when_missing() {
        let ctx = RouteContext::new("dev".into(), "acme".into(), None);
        let config = Config::with_base_url(DEFAULT_DIRECT_LINE_BASE);
        assert!(config.resolve_secret_with(&ctx, |_key| None).is_none());
    }

    #[test]
    fn allows_custom_base_url() {
        let config = Config::with_base_url("https://example.com/directline");
        assert_eq!(config.direct_line_base(), "https://example.com/directline");
    }

    #[test]
    fn resolves_oauth_config_for_tenant() {
        let ctx = TenantCtx::new(
            greentic_types::EnvId::from("dev"),
            greentic_types::TenantId::from("acme"),
        );
        let mut raw = HashMap::new();
        raw.insert(
            "WEBCHAT_OAUTH_ISSUER__DEV__ACME".to_string(),
            "https://oauth.dev.greentic.io".to_string(),
        );
        raw.insert(
            "WEBCHAT_OAUTH_CLIENT_ID__DEV__ACME".to_string(),
            "client-123".to_string(),
        );
        raw.insert(
            "WEBCHAT_OAUTH_REDIRECT_BASE__DEV__ACME".to_string(),
            "https://messaging.dev.greentic.io".to_string(),
        );
        let map = Arc::new(raw);

        let config = Config::from_env().with_oauth_lookup({
            let map = Arc::clone(&map);
            move |key| map.get(key).cloned()
        });
        let resolved = config
            .resolve_oauth_config(&ctx)
            .expect("expected oauth config");
        assert_eq!(resolved.issuer, "https://oauth.dev.greentic.io");
        assert_eq!(resolved.client_id, "client-123");
        assert_eq!(resolved.redirect_base, "https://messaging.dev.greentic.io");
    }

    #[test]
    fn resolves_team_specific_oauth_config() {
        let ctx = TenantCtx::new(
            greentic_types::EnvId::from("dev"),
            greentic_types::TenantId::from("acme"),
        )
        .with_team(Some(greentic_types::TeamId::from("support")));

        let mut raw = HashMap::new();
        raw.insert(
            "WEBCHAT_OAUTH_ISSUER__DEV__ACME__SUPPORT".to_string(),
            "https://team-oauth.dev.greentic.io".to_string(),
        );
        raw.insert(
            "WEBCHAT_OAUTH_CLIENT_ID__DEV__ACME__SUPPORT".to_string(),
            "team-client-456".to_string(),
        );
        raw.insert(
            "WEBCHAT_OAUTH_REDIRECT_BASE__DEV__ACME__SUPPORT".to_string(),
            "https://team-messaging.dev.greentic.io".to_string(),
        );
        let map = Arc::new(raw);

        let config = Config::from_env().with_oauth_lookup({
            let map = Arc::clone(&map);
            move |key| map.get(key).cloned()
        });
        let resolved = config
            .resolve_oauth_config(&ctx)
            .expect("expected oauth config");
        assert_eq!(resolved.issuer, "https://team-oauth.dev.greentic.io");
        assert_eq!(resolved.client_id, "team-client-456");
        assert_eq!(
            resolved.redirect_base,
            "https://team-messaging.dev.greentic.io"
        );
    }

    #[test]
    fn supports_legacy_oauth_keys() {
        let ctx = TenantCtx::new(
            greentic_types::EnvId::from("dev"),
            greentic_types::TenantId::from("acme"),
        )
        .with_team(Some(greentic_types::TeamId::from("support")));

        let mut raw = HashMap::new();
        raw.insert(
            "OAUTH_ISSUER__DEV__ACME__SUPPORT".to_string(),
            "https://legacy-oauth.dev.greentic.io".to_string(),
        );
        raw.insert(
            "OAUTH_CLIENT_ID__DEV__ACME__SUPPORT".to_string(),
            "legacy-client".to_string(),
        );
        raw.insert(
            "OAUTH_REDIRECT_BASE__DEV__ACME__SUPPORT".to_string(),
            "https://legacy-messaging.dev.greentic.io".to_string(),
        );

        let map = Arc::new(raw);
        let config = Config::from_env().with_oauth_lookup({
            let map = Arc::clone(&map);
            move |key| map.get(key).cloned()
        });
        let resolved = config
            .resolve_oauth_config(&ctx)
            .expect("expected oauth config");
        assert_eq!(resolved.issuer, "https://legacy-oauth.dev.greentic.io");
        assert_eq!(resolved.client_id, "legacy-client");
        assert_eq!(
            resolved.redirect_base,
            "https://legacy-messaging.dev.greentic.io"
        );
    }
}

#[derive(Clone, Debug)]
pub struct OAuthProviderConfig {
    pub issuer: String,
    pub client_id: String,
    pub redirect_base: String,
}
