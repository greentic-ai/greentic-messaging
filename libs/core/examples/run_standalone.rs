use greentic_secrets::spec::{
    Scope, SecretUri, SecretVersion, SecretsBackend, VersionedSecret, helpers::record_from_plain,
};
use gsm_core::platforms::webchat::{
    config::Config,
    standalone::{StandaloneState, router},
};
use http::{HeaderName, HeaderValue, Method};
use std::{fs::OpenOptions, sync::Arc, time::Duration};
use tower_http::cors::{Any, CorsLayer};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _guard = init_logging()?;
    let config = Config::default();
    let signing_secret = match std::env::var("WEBCHAT_JWT_SIGNING_KEY") {
        Ok(value) => {
            tracing::info!("WEBCHAT_JWT_SIGNING_KEY detected");
            value
        }
        Err(_) => {
            tracing::warn!("WEBCHAT_JWT_SIGNING_KEY not set; using local-dev-secret");
            "local-dev-secret".into()
        }
    };
    let secrets = Arc::new(StaticSecretsBackend::new(signing_secret));
    let signing_scope = Scope::new("global", "webchat", None)?;
    let provider = gsm_core::platforms::webchat::WebChatProvider::new(config, secrets)
        .with_signing_scope(signing_scope);
    let state = Arc::new(StandaloneState::new(provider).await?);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8090").await?;
    let (cors_layer, cors_summary) = cors_layer_from_env();
    tracing::info!(
        "CORS configuration loaded: origins={:?} headers={:?} methods={:?} max_age={}s",
        cors_summary.allowed_origins,
        cors_summary.allowed_headers,
        cors_summary.allowed_methods,
        cors_summary.max_age
    );
    let app = router(Arc::clone(&state)).layer(cors_layer);
    axum::serve(listener, app.into_make_service()).await?;
    Ok(())
}

#[derive(Debug)]
struct CorsConfigSummary {
    allowed_origins: Vec<String>,
    allowed_headers: Vec<String>,
    allowed_methods: Vec<String>,
    max_age: u64,
}

fn cors_layer_from_env() -> (CorsLayer, CorsConfigSummary) {
    let origin_env = std::env::var("WEBCHAT_CORS_ALLOWED")
        .or_else(|_| std::env::var("CORS_ALLOWED_ORIGINS"))
        .unwrap_or_else(|_| "http://localhost:5174".into());
    let (origin_strings, origin_values) =
        split_and_parse(&origin_env, |value| value.parse::<HeaderValue>().ok());

    let header_env = std::env::var("CORS_ALLOWED_HEADERS")
        .unwrap_or_else(|_| "Authorization,Content-Type,x-ms-bot-agent,x-requested-with".into());
    let (header_strings, headers) =
        split_and_parse(&header_env, |value| value.parse::<HeaderName>().ok());

    let method_env =
        std::env::var("CORS_ALLOWED_METHODS").unwrap_or_else(|_| "GET,POST,OPTIONS".into());
    let (method_strings, mut methods) = split_and_parse(&method_env, |value| {
        Method::from_bytes(value.as_bytes()).ok()
    });

    let max_age = std::env::var("CORS_MAX_AGE")
        .ok()
        .and_then(|val| val.parse::<u64>().ok())
        .unwrap_or(600);

    if methods.is_empty() {
        methods = vec![Method::GET, Method::POST, Method::OPTIONS];
    }

    let mut layer = CorsLayer::new()
        .allow_headers(headers)
        .allow_methods(methods.clone())
        .max_age(Duration::from_secs(max_age));

    layer = if origin_values.is_empty() {
        layer.allow_origin(Any)
    } else {
        layer.allow_origin(origin_values)
    };

    let summary = CorsConfigSummary {
        allowed_origins: if origin_strings.is_empty() {
            vec!["* (any)".into()]
        } else {
            origin_strings
        },
        allowed_headers: header_strings,
        allowed_methods: if method_strings.is_empty() {
            vec!["GET".into(), "POST".into(), "OPTIONS".into()]
        } else {
            method_strings
        },
        max_age,
    };

    (layer, summary)
}

fn split_and_parse<T, F>(input: &str, mut parse: F) -> (Vec<String>, Vec<T>)
where
    F: FnMut(&str) -> Option<T>,
{
    let mut raw_values = Vec::new();
    let mut parsed_values = Vec::new();
    for segment in input.split(',') {
        let trimmed = segment.trim();
        if trimmed.is_empty() {
            continue;
        }
        raw_values.push(trimmed.to_string());
        if let Some(value) = parse(trimmed) {
            parsed_values.push(value);
        }
    }
    (raw_values, parsed_values)
}

fn init_logging() -> anyhow::Result<WorkerGuard> {
    let log_path =
        std::env::var("WEBCHAT_LOG_FILE").unwrap_or_else(|_| "webchat-standalone.log".into());
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    let (writer, guard) = tracing_appender::non_blocking(file);

    let filter =
        tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into());
    tracing_subscriber::registry()
        .with(filter)
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(writer)
                .with_ansi(false),
        )
        .init();

    tracing::info!("webchat standalone logging to {}", log_path);
    Ok(guard)
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
