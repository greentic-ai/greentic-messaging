//! WhatsApp ingress adapter: verifies Meta webhook signatures, normalizes
//! messages into `MessageEnvelope`s, and publishes them to NATS.
//!
//! ```text
//! Meta calls `/ingress/whatsapp/{tenant}`; verified messages are emitted on
//! `greentic.msg.in.{tenant}.whatsapp.{from}`.
//! ```

use anyhow::Result;
use async_nats::Client as Nats;
use axum::{
    Extension, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::get,
};
use gsm_core::platforms::whatsapp::{creds::WhatsAppCredentials, provision::ensure_subscription};
use gsm_core::{
    DefaultResolver, MessageEnvelope, NodeResult, Platform, Provider, ProviderKey,
    ProviderRegistry, SecretsResolver, TenantCtx, in_subject, make_tenant_ctx,
};
use gsm_dlq::{DlqError, DlqPublisher};
use gsm_idempotency::{IdKey as IdemKey, IdempotencyGuard};
use gsm_ingress_common::{init_guard, record_idempotency_hit, record_ingress, start_ingress_span};
use gsm_telemetry::{install as init_telemetry, set_current_tenant_ctx};
use hmac::{Hmac, Mac};
use security::middleware::{ActionContext, SharedActionContext, handle_action};
use serde::Deserialize;
use serde_json::Value;
use sha2::Sha256;
use std::sync::Arc;
use time::OffsetDateTime;

#[cfg(test)]
mod test_support {
    use super::*;
    use gsm_core::{NodeError, SecretPath};
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[allow(dead_code)]
    #[derive(Default)]
    pub(super) struct InMemorySecrets {
        store: Mutex<HashMap<String, Value>>,
    }

    #[async_trait::async_trait]
    impl SecretsResolver for InMemorySecrets {
        async fn get_json<T>(&self, path: &SecretPath, _ctx: &TenantCtx) -> NodeResult<Option<T>>
        where
            T: serde::de::DeserializeOwned + Send,
        {
            let value = self.store.lock().unwrap().get(path.as_str()).cloned();
            if let Some(json) = value {
                serde_json::from_value(json).map(Some).map_err(|err| {
                    NodeError::new("decode", "failed to decode secret").with_source(err)
                })
            } else {
                Ok(None)
            }
        }

        async fn put_json<T>(
            &self,
            path: &SecretPath,
            _ctx: &TenantCtx,
            value: &T,
        ) -> NodeResult<()>
        where
            T: serde::Serialize + Sync + Send,
        {
            let json = serde_json::to_value(value).map_err(|err| {
                NodeError::new("encode", "failed to encode secret").with_source(err)
            })?;
            self.store
                .lock()
                .unwrap()
                .insert(path.as_str().to_string(), json);
            Ok(())
        }
    }
}

type HmacSha256 = Hmac<Sha256>;

struct AppState<R>
where
    R: SecretsResolver + Send + Sync + 'static,
{
    nats: Nats,
    registry: Arc<ProviderRegistry<WhatsAppProvider>>,
    resolver: Arc<R>,
    http_client: Arc<reqwest::Client>,
    webhook_base: String,
    api_base: String,
    idem_guard: IdempotencyGuard,
    dlq: DlqPublisher,
}

impl<R> Clone for AppState<R>
where
    R: SecretsResolver + Send + Sync + 'static,
{
    fn clone(&self) -> Self {
        Self {
            nats: self.nats.clone(),
            registry: self.registry.clone(),
            resolver: self.resolver.clone(),
            http_client: self.http_client.clone(),
            webhook_base: self.webhook_base.clone(),
            api_base: self.api_base.clone(),
            idem_guard: self.idem_guard.clone(),
            dlq: self.dlq.clone(),
        }
    }
}

#[derive(Clone)]
struct WhatsAppProvider {
    creds: WhatsAppCredentials,
}

impl Provider for WhatsAppProvider {}

#[tokio::main]
async fn main() -> Result<()> {
    init_telemetry("greentic-messaging")?;
    let nats_url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
    let webhook_base =
        std::env::var("WHATSAPP_WEBHOOK_BASE").unwrap_or_else(|_| "http://localhost:8087".into());
    let api_base =
        std::env::var("WA_API_BASE").unwrap_or_else(|_| "https://graph.facebook.com".into());

    let nats = async_nats::connect(nats_url).await?;
    let idem_guard = init_guard(&nats).await?;
    let dlq = DlqPublisher::new("ingress", nats.clone()).await?;
    let registry = Arc::new(ProviderRegistry::new());
    let resolver = Arc::new(DefaultResolver::new().await?);
    let http_client = Arc::new(reqwest::Client::new());

    let state = AppState {
        nats,
        registry,
        resolver,
        http_client,
        webhook_base,
        api_base,
        idem_guard,
        dlq,
    };

    let mut app: Router<AppState<DefaultResolver>> = Router::new()
        .route(
            "/ingress/whatsapp/{tenant}",
            get(verify::<DefaultResolver>).post(receive::<DefaultResolver>),
        )
        .route("/healthz", get(healthz));

    match ActionContext::from_env(&state.nats).await {
        Ok(ctx) => {
            let shared: SharedActionContext = std::sync::Arc::new(ctx);
            app = app
                .route("/a", get(handle_action).layer(Extension(shared.clone())))
                .route("/a/whatsapp", get(handle_action).layer(Extension(shared)));
        }
        Err(err) => {
            tracing::warn!(error = %err, "action links disabled for ingress-whatsapp");
        }
    }

    let app = app.with_state(state);

    let addr: std::net::SocketAddr = std::env::var("BIND")
        .unwrap_or_else(|_| "0.0.0.0:8087".into())
        .parse()
        .unwrap();
    tracing::info!("ingress-whatsapp listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app.into_make_service()).await?;
    Ok(())
}

#[derive(Deserialize)]
struct VerifyQs {
    #[serde(rename = "hub.mode")]
    mode: Option<String>,
    #[serde(rename = "hub.challenge")]
    challenge: Option<String>,
    #[serde(rename = "hub.verify_token")]
    token: Option<String>,
}

async fn verify<R>(
    State(state): State<AppState<R>>,
    Path(tenant): Path<String>,
    Query(q): Query<VerifyQs>,
) -> impl IntoResponse
where
    R: SecretsResolver + Send + Sync + 'static,
{
    let ctx = make_tenant_ctx(tenant.clone(), None, None);
    let provider = match resolve_provider(&state, &ctx, &tenant).await {
        Ok(provider) => provider,
        Err(err) => {
            tracing::error!(error = %err, tenant = %ctx.tenant, "failed to resolve whatsapp provider");
            return (StatusCode::INTERNAL_SERVER_ERROR, String::new());
        }
    };

    if q.mode.as_deref() == Some("subscribe")
        && q.token.as_deref() == Some(provider.creds.verify_token.as_str())
    {
        (StatusCode::OK, q.challenge.unwrap_or_default())
    } else {
        (StatusCode::FORBIDDEN, "forbidden".to_string())
    }
}

async fn receive<R>(
    State(state): State<AppState<R>>,
    Path(tenant): Path<String>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> impl IntoResponse
where
    R: SecretsResolver + Send + Sync + 'static,
{
    let ctx = make_tenant_ctx(tenant.clone(), None, None);
    let provider = match resolve_provider(&state, &ctx, &tenant).await {
        Ok(provider) => provider,
        Err(err) => {
            tracing::error!(error = %err, tenant = %ctx.tenant, "failed to resolve whatsapp provider");
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
    };

    if !verify_fb_sig(&provider.creds.app_secret, &headers, &body) {
        tracing::warn!("invalid whatsapp signature");
        return StatusCode::UNAUTHORIZED;
    }

    let payload: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("failed to decode payload: {e}");
            return StatusCode::BAD_REQUEST;
        }
    };

    let envelopes = extract_envelopes(ctx.tenant.as_str(), &payload);
    for env in envelopes {
        let span = start_ingress_span(&env);
        let _guard = span.enter();
        let subject = in_subject(env.tenant.as_str(), env.platform.as_str(), &env.chat_id);
        let key = IdemKey {
            tenant: env.tenant.clone(),
            platform: env.platform.as_str().to_string(),
            msg_id: env.msg_id.clone(),
        };
        match state.idem_guard.should_process(&key).await {
            Ok(true) => {}
            Ok(false) => {
                record_idempotency_hit(&key.tenant);
                tracing::info!(
                    tenant = %key.tenant,
                    platform = %key.platform,
                    msg_id = %key.msg_id,
                    "duplicate whatsapp event dropped"
                );
                continue;
            }
            Err(err) => {
                tracing::error!(
                    error = %err,
                    tenant = %key.tenant,
                    platform = %key.platform,
                    msg_id = %key.msg_id,
                    "idempotency check failed; continuing"
                );
            }
        }
        let invocation = match env.clone().into_invocation() {
            Ok(envelope) => envelope,
            Err(err) => {
                tracing::error!(error = %err, "failed to build invocation envelope");
                return StatusCode::INTERNAL_SERVER_ERROR;
            }
        };
        set_current_tenant_ctx(invocation.ctx.clone());

        let payload = match serde_json::to_vec(&invocation) {
            Ok(bytes) => bytes,
            Err(err) => {
                tracing::error!(error = %err, "failed to serialise invocation");
                return StatusCode::INTERNAL_SERVER_ERROR;
            }
        };

        if let Err(e) = state.nats.publish(subject.clone(), payload.into()).await {
            tracing::error!("publish failed on {subject}: {e}");
            if let Err(dlq_err) = state
                .dlq
                .publish(
                    env.tenant.as_str(),
                    env.platform.as_str(),
                    &env.msg_id,
                    1,
                    DlqError {
                        code: "E_PUBLISH".into(),
                        message: e.to_string(),
                        stage: None,
                    },
                    &invocation,
                )
                .await
            {
                tracing::error!("failed to publish dlq entry: {dlq_err}");
            }
            return StatusCode::INTERNAL_SERVER_ERROR;
        } else {
            record_ingress(&env);
        }
    }

    StatusCode::OK
}

async fn healthz() -> impl IntoResponse {
    StatusCode::NO_CONTENT
}

async fn resolve_provider<R>(
    state: &AppState<R>,
    ctx: &TenantCtx,
    tenant: &str,
) -> NodeResult<Arc<WhatsAppProvider>>
where
    R: SecretsResolver + Send + Sync + 'static,
{
    let key = ProviderKey {
        platform: Platform::WhatsApp,
        env: ctx.env.clone(),
        tenant: ctx.tenant.clone(),
        team: None,
    };

    if let Some(existing) = state.registry.get(&key) {
        return Ok(existing);
    }

    let target_url = format!(
        "{}/ingress/whatsapp/{}",
        state.webhook_base.trim_end_matches('/'),
        tenant
    );

    let creds = ensure_subscription(
        &state.http_client,
        ctx,
        &target_url,
        &state.api_base,
        state.resolver.as_ref(),
    )
    .await?;

    let provider = Arc::new(WhatsAppProvider { creds });
    state.registry.put(key, provider.clone());
    Ok(provider)
}

fn verify_fb_sig(app_secret: &str, headers: &HeaderMap, body: &[u8]) -> bool {
    let sig = headers
        .get("X-Hub-Signature-256")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !sig.starts_with("sha256=") {
        return false;
    }
    let provided = &sig[7..];
    let mut mac = match HmacSha256::new_from_slice(app_secret.as_bytes()) {
        Ok(mac) => mac,
        Err(_) => return false,
    };
    mac.update(body);
    let digest = mac.finalize().into_bytes();
    constant_time_eq(provided.as_bytes(), hex_encode(&digest).as_bytes())
}

fn extract_envelopes(tenant: &str, value: &Value) -> Vec<MessageEnvelope> {
    let mut out = Vec::new();
    let entries = match value.get("entry").and_then(|v| v.as_array()) {
        Some(entries) => entries,
        None => return out,
    };

    for entry in entries {
        let changes = match entry.get("changes").and_then(|v| v.as_array()) {
            Some(changes) => changes,
            None => continue,
        };
        for change in changes {
            let Some(value) = change.get("value") else {
                continue;
            };
            let Some(messages) = value.get("messages").and_then(|v| v.as_array()) else {
                continue;
            };
            for message in messages {
                if let Some(env) = envelope_from_message(tenant, message) {
                    out.push(env);
                }
            }
        }
    }
    out
}

fn envelope_from_message(tenant: &str, message: &Value) -> Option<MessageEnvelope> {
    let from = message.get("from")?.as_str()?.to_string();
    let chat_id = from.clone();
    let text = message
        .get("text")
        .and_then(|t| t.get("body"))
        .and_then(|b| b.as_str())
        .map(|s| s.to_string());
    let ts_str = message
        .get("timestamp")
        .and_then(|v| v.as_str())
        .unwrap_or("0");
    let ts = ts_str.parse::<i64>().ok();
    let timestamp = ts
        .and_then(|secs| OffsetDateTime::from_unix_timestamp(secs).ok())
        .unwrap_or_else(OffsetDateTime::now_utc)
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".into());

    Some(MessageEnvelope {
        tenant: tenant.to_string(),
        platform: Platform::WhatsApp,
        chat_id,
        user_id: from,
        thread_id: None,
        msg_id: format!("wa:{ts_str}"),
        text,
        timestamp,
        context: Default::default(),
    })
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn verify_fb_sig_accepts_valid_signature() {
        let secret = "secret";
        let body = b"{\"entry\":[]}";
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        let digest = mac.finalize().into_bytes();
        let sig = format!("sha256={}", hex_encode(&digest));

        let mut headers = HeaderMap::new();
        headers.insert("X-Hub-Signature-256", HeaderValue::from_str(&sig).unwrap());
        assert!(verify_fb_sig(secret, &headers, body));
    }

    #[test]
    fn verify_fb_sig_rejects_bad_signature() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "X-Hub-Signature-256",
            HeaderValue::from_static("sha256=deadbeef"),
        );
        assert!(!verify_fb_sig("secret", &headers, b"{}"));
    }

    #[test]
    fn extract_envelopes_returns_message() {
        let sample = serde_json::json!({
            "entry": [
                {"changes": [
                    {"value": {
                        "contacts": [],
                        "messages": [
                            {
                                "from": "12345",
                                "timestamp": "1700000000",
                                "text": {"body": "Hi"}
                            }
                        ]
                    }}
                ]}
            ]
        });
        let envs = extract_envelopes("tenant", &sample);
        assert_eq!(envs.len(), 1);
        let env = &envs[0];
        assert_eq!(env.chat_id, "12345");
        assert_eq!(env.text.as_deref(), Some("Hi"));
        assert_eq!(env.platform, Platform::WhatsApp);
    }

    #[test]
    fn invocation_ctx_includes_tenant_and_user() {
        let sample = serde_json::json!({
            "entry": [
                {"changes": [
                    {"value": {
                        "messages": [
                            {
                                "from": "5551234",
                                "timestamp": "1700000000",
                                "text": {"body": "Hello"}
                            }
                        ]
                    }}
                ]}
            ]
        });
        let env = extract_envelopes("acme", &sample)
            .pop()
            .expect("message envelope");
        let invocation = env.clone().into_invocation().expect("invocation");
        assert_eq!(invocation.ctx.tenant.as_str(), "acme");
        assert_eq!(invocation.ctx.team, None);
        assert_eq!(
            invocation.ctx.user.as_ref().map(|u| u.as_str()),
            Some("5551234")
        );
    }
}
