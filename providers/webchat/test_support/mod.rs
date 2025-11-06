#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use greentic_messaging_providers_webchat::WebChatProvider;
use greentic_messaging_providers_webchat::config::Config;
use greentic_secrets::spec::{
    Scope, SecretUri, SecretsBackend, VersionedSecret, helpers::record_from_plain,
};
use greentic_types::{EnvId, TeamId, TenantCtx, TenantId};

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
        _record: greentic_secrets::spec::SecretRecord,
    ) -> greentic_secrets::spec::Result<greentic_secrets::spec::SecretVersion> {
        unimplemented!("test backend does not support put")
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
        _scope: &greentic_secrets::spec::Scope,
        _category_prefix: Option<&str>,
        _name_prefix: Option<&str>,
    ) -> greentic_secrets::spec::Result<Vec<greentic_secrets::spec::SecretListItem>> {
        unimplemented!("test backend does not support list")
    }

    fn delete(
        &self,
        _uri: &SecretUri,
    ) -> greentic_secrets::spec::Result<greentic_secrets::spec::SecretVersion> {
        unimplemented!("test backend does not support delete")
    }

    fn versions(
        &self,
        _uri: &SecretUri,
    ) -> greentic_secrets::spec::Result<Vec<greentic_secrets::spec::SecretVersion>> {
        unimplemented!("test backend does not support versions")
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
    let mut ctx = TenantCtx::new(EnvId::from(env), TenantId::from(tenant));
    if let Some(team) = team {
        ctx = ctx.with_team(Some(TeamId::from(team)));
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
