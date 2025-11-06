use greentic_secrets::spec::{
    Scope, SecretUri, SecretVersion, SecretsBackend, VersionedSecret, helpers::record_from_plain,
};
use gsm_core::platforms::webchat::{
    config::Config,
    standalone::{StandaloneState, router},
};
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::default();
    let signing_secret =
        std::env::var("WEBCHAT_JWT_SIGNING_KEY").unwrap_or_else(|_| "local-dev-secret".into());
    let secrets = Arc::new(StaticSecretsBackend::new(signing_secret));
    let signing_scope = Scope::new("global", "webchat", None)?;
    let provider = gsm_core::platforms::webchat::WebChatProvider::new(config, secrets)
        .with_signing_scope(signing_scope);
    let state = Arc::new(StandaloneState::new(provider).await?);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8090").await?;
    let app = router(Arc::clone(&state));
    axum::serve(listener, app.into_make_service()).await?;
    Ok(())
}

#[derive(Clone)]
struct StaticSecretsBackend {
    secret: String,
}

impl StaticSecretsBackend {
    fn new(secret: String) -> Self {
        Self { secret }
    }
}

impl SecretsBackend for StaticSecretsBackend {
    fn put(
        &self,
        _record: greentic_secrets::spec::SecretRecord,
    ) -> greentic_secrets::spec::Result<greentic_secrets::spec::SecretVersion> {
        unimplemented!("static backend does not support write operations")
    }

    fn get(
        &self,
        uri: &SecretUri,
        _version: Option<u64>,
    ) -> greentic_secrets::spec::Result<Option<VersionedSecret>> {
        if uri.category() == "webchat" && uri.name() == "jwt_signing_key" {
            let record = record_from_plain(self.secret.clone());
            Ok(Some(VersionedSecret {
                version: 1,
                deleted: false,
                record: Some(record),
            }))
        } else {
            Ok(None)
        }
    }

    fn list(
        &self,
        _scope: &Scope,
        _category_prefix: Option<&str>,
        _name_prefix: Option<&str>,
    ) -> greentic_secrets::spec::Result<Vec<greentic_secrets::spec::SecretListItem>> {
        unimplemented!("static backend does not support list operations")
    }

    fn delete(&self, _uri: &SecretUri) -> greentic_secrets::spec::Result<SecretVersion> {
        unimplemented!("static backend does not support delete operations")
    }

    fn versions(&self, _uri: &SecretUri) -> greentic_secrets::spec::Result<Vec<SecretVersion>> {
        unimplemented!("static backend does not support version operations")
    }

    fn exists(&self, uri: &SecretUri) -> greentic_secrets::spec::Result<bool> {
        Ok(uri.category() == "webchat" && uri.name() == "jwt_signing_key")
    }
}
