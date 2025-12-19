#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use greentic_secrets::spec::{
    Scope, SecretUri, SecretVersion, SecretsBackend, VersionedSecret, helpers::record_from_plain,
};
use greentic_types::{EnvId, TeamId, TenantCtx, TenantId};
use gsm_core::platforms::webchat::config::Config;
use gsm_core::platforms::webchat::provider::WebChatProvider;

#[derive(Clone, Default)]
pub struct TestSecretsBackend {
    inner: Arc<Mutex<HashMap<String, VersionedSecret>>>,
}

impl TestSecretsBackend {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert_secret(&self, scope: Scope, category: &str, name: &str, value: &str) {
        let uri = SecretUri::new(scope, category.to_string(), name.to_string())
            .expect("valid secret uri");
        let record = record_from_plain(value.to_string());
        let secret = VersionedSecret {
            version: 1,
            deleted: false,
            record: Some(record),
        };
        self.inner
            .lock()
            .expect("lock secrets map")
            .insert(uri.to_string(), secret);
    }

    pub fn backend_arc(self) -> Arc<Self> {
        Arc::new(self)
    }
}

impl SecretsBackend for TestSecretsBackend {
    fn put(
        &self,
        record: greentic_secrets::spec::SecretRecord,
    ) -> greentic_secrets::spec::Result<greentic_secrets::spec::SecretVersion> {
        self.inner.lock().expect("lock secrets map").insert(
            record.meta.uri.to_string(),
            VersionedSecret {
                version: 1,
                deleted: false,
                record: Some(record),
            },
        );
        Ok(greentic_secrets::spec::SecretVersion {
            version: 1,
            deleted: false,
        })
    }

    fn get(
        &self,
        uri: &SecretUri,
        _version: Option<u64>,
    ) -> greentic_secrets::spec::Result<Option<VersionedSecret>> {
        Ok(self
            .inner
            .lock()
            .expect("lock secrets map")
            .get(&uri.to_string())
            .cloned())
    }

    fn list(
        &self,
        scope: &Scope,
        category_prefix: Option<&str>,
        name_prefix: Option<&str>,
    ) -> greentic_secrets::spec::Result<Vec<greentic_secrets::spec::SecretListItem>> {
        let items = self
            .inner
            .lock()
            .expect("lock secrets map")
            .values()
            .filter_map(|secret| secret.record.as_ref())
            .filter(|record| {
                record.meta.uri.scope() == scope
                    && category_prefix.is_none_or(|p| record.meta.uri.category().starts_with(p))
                    && name_prefix.is_none_or(|p| record.meta.uri.name().starts_with(p))
            })
            .map(|record| {
                greentic_secrets::spec::SecretListItem::from_meta(&record.meta, Some("1".into()))
            })
            .collect();
        Ok(items)
    }

    fn delete(&self, uri: &SecretUri) -> greentic_secrets::spec::Result<SecretVersion> {
        let removed = self
            .inner
            .lock()
            .expect("lock secrets map")
            .remove(&uri.to_string());
        match removed {
            Some(secret) => Ok(SecretVersion {
                version: secret.version,
                deleted: secret.deleted,
            }),
            None => Err(greentic_secrets::spec::Error::NotFound {
                entity: uri.to_string(),
            }),
        }
    }

    fn versions(&self, uri: &SecretUri) -> greentic_secrets::spec::Result<Vec<SecretVersion>> {
        Ok(self
            .inner
            .lock()
            .expect("lock secrets map")
            .get(&uri.to_string())
            .map(|secret| {
                vec![SecretVersion {
                    version: secret.version,
                    deleted: secret.deleted,
                }]
            })
            .unwrap_or_default())
    }

    fn exists(&self, uri: &SecretUri) -> greentic_secrets::spec::Result<bool> {
        Ok(self
            .inner
            .lock()
            .expect("lock secrets map")
            .contains_key(&uri.to_string()))
    }
}

pub fn tenant_scope(env: &str, tenant: &str, team: Option<&str>) -> Scope {
    Scope::new(
        env.to_ascii_lowercase(),
        tenant.to_ascii_lowercase(),
        team.map(|value| value.to_ascii_lowercase()),
    )
    .expect("valid tenant scope")
}

pub fn signing_scope() -> Scope {
    Scope::new("global", "webchat", None).expect("valid signing scope")
}

pub fn tenant_ctx(env: &str, tenant: &str, team: Option<&str>) -> TenantCtx {
    let mut ctx = TenantCtx::new(EnvId(env.to_string()), TenantId(tenant.to_string()));
    if let Some(team) = team {
        ctx = ctx.with_team(Some(TeamId(team.to_string())));
    }
    ctx
}

pub fn provider_with_secrets(
    config: Config,
    signing_scope: Scope,
    secrets: &[(&Scope, &str, &str, &str)],
) -> WebChatProvider {
    let backend = TestSecretsBackend::new();
    backend.insert_secret(
        signing_scope.clone(),
        "webchat",
        "jwt_signing_key",
        "test-signing-key",
    );
    for (scope, category, name, value) in secrets {
        backend.insert_secret((**scope).clone(), category, name, value);
    }
    let provider = WebChatProvider::new(config, backend.backend_arc());
    provider.with_signing_scope(signing_scope)
}
