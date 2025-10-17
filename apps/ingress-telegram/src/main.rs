//! Telegram ingress adapter: validates shared secrets, normalizes updates into
//! `MessageEnvelope`s, and publishes them to tenant-specific NATS subjects.
//!
//! ```text
//! Telegram POSTs updates to `/telegram/webhook`; the payload is deserialized
//! into `TelegramUpdate` and re-published to NATS.
//! ```

mod config;
mod reconciler;
mod secrets;
mod telegram_api;

use anyhow::Result;
use async_nats::Client as Nats;
use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    middleware,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use gsm_core::*;
use gsm_dlq::{DlqError, DlqPublisher};
use gsm_idempotency::IdKey as IdemKey;
use gsm_ingress_common::{
    ack202, init_guard, rate_limit_layer, record_idempotency_hit, record_ingress,
    start_ingress_span, verify_bearer, verify_hmac, with_request_id,
};
use config::load_tenants;
use gsm_telemetry::{init_telemetry, TelemetryConfig};
use reconciler::{
    allowed_updates, desired_webhook_url, ensure_secret, reconcile_all_telegram_webhooks,
    urls_match,
};
use reqwest::Client;
use secrets::{EnvSecretsManager, SecretsManager};
use security::middleware::{handle_action, ActionContext, SharedActionContext};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use telegram_api::{HttpTelegramApi, TelegramApi, WebhookInfo};
use time::OffsetDateTime;

#[derive(Clone)]
struct AppState {
    nats: Nats,
    tenant: String,
    secret_token: Option<String>,
    idem_guard: gsm_idempotency::IdempotencyGuard,
    dlq: DlqPublisher,
    tenants: Vec<config::Tenant>,
    secrets: Arc<dyn SecretsManager>,
    telegram_api: Arc<dyn TelegramApi>,
}

#[derive(Serialize)]
struct ErrorResponse {
    ok: bool,
    error: String,
}

#[derive(Serialize)]
struct RegisterResponse {
    ok: bool,
    tenant: String,
    applied: bool,
    url: String,
    allowed_updates: Vec<String>,
}

#[derive(Serialize)]
struct DeregisterResponse {
    ok: bool,
    tenant: String,
}

#[derive(Serialize)]
struct StatusResponse {
    ok: bool,
    tenant: String,
    desired_url: String,
    matches: bool,
    info: WebhookInfo,
}

fn error_response(
    status: StatusCode,
    message: impl Into<String>,
) -> (StatusCode, Json<ErrorResponse>) {
    (
        status,
        Json(ErrorResponse {
            ok: false,
            error: message.into(),
        }),
    )
}

#[derive(Debug)]
enum AdminOpError {
    TenantNotFound(String),
    TelegramDisabled(String),
    MissingBotToken(String),
    Secret(anyhow::Error),
    Telegram(anyhow::Error),
}

fn map_admin_error(err: AdminOpError) -> (StatusCode, Json<ErrorResponse>) {
    match err {
        AdminOpError::TenantNotFound(tenant) => error_response(
            StatusCode::NOT_FOUND,
            format!("tenant {} not found", tenant),
        ),
        AdminOpError::TelegramDisabled(tenant) => error_response(
            StatusCode::BAD_REQUEST,
            format!("telegram disabled or not configured for {}", tenant),
        ),
        AdminOpError::MissingBotToken(tenant) => error_response(
            StatusCode::BAD_REQUEST,
            format!("bot token not configured for {}", tenant),
        ),
        AdminOpError::Secret(err) => {
            error_response(StatusCode::INTERNAL_SERVER_ERROR, format!("secret error: {}", err))
        }
        AdminOpError::Telegram(err) => {
            error_response(StatusCode::BAD_GATEWAY, format!("telegram error: {}", err))
        }
    }
}

async fn register_tenant_op(
    tenants: Vec<config::Tenant>,
    secrets: Arc<dyn SecretsManager>,
    api: Arc<dyn TelegramApi>,
    tenant_id: String,
) -> Result<RegisterResponse, AdminOpError> {
    let tenant = tenants
        .into_iter()
        .find(|t| t.id == tenant_id)
        .ok_or_else(|| AdminOpError::TenantNotFound(tenant_id.clone()))?;
    let tenant_id = tenant.id.clone();
    let telegram_cfg = tenant
        .telegram
        .clone()
        .filter(|cfg| cfg.enabled)
        .ok_or_else(|| AdminOpError::TelegramDisabled(tenant_id.clone()))?;
    let bot_token_key = format!("tenants/{}/telegram/bot_token", tenant_id);
    let bot_token = secrets
        .get(&bot_token_key)
        .await
        .map_err(AdminOpError::Secret)?
        .ok_or_else(|| AdminOpError::MissingBotToken(tenant_id.clone()))?;
    let secret = ensure_secret(secrets.as_ref(), &tenant_id, &telegram_cfg)
        .await
        .map_err(AdminOpError::Secret)?;
    let desired_url = desired_webhook_url(&telegram_cfg, &tenant_id);
    let allowed = allowed_updates(&telegram_cfg);
    api.set_webhook(&bot_token, &desired_url, &secret, &allowed, false)
        .await
        .map_err(AdminOpError::Telegram)?;
    tracing::info!(
        event = "telegram_admin_register",
        tenant = %tenant_id,
        url = %desired_url
    );
    Ok(RegisterResponse {
        ok: true,
        tenant: tenant_id,
        applied: true,
        url: desired_url,
        allowed_updates: allowed,
    })
}

async fn deregister_tenant_op(
    tenants: Vec<config::Tenant>,
    secrets: Arc<dyn SecretsManager>,
    api: Arc<dyn TelegramApi>,
    tenant_id: String,
) -> Result<DeregisterResponse, AdminOpError> {
    let tenant = tenants
        .into_iter()
        .find(|t| t.id == tenant_id)
        .ok_or_else(|| AdminOpError::TenantNotFound(tenant_id.clone()))?;
    let tenant_id = tenant.id.clone();
    if tenant
        .telegram
        .as_ref()
        .filter(|cfg| cfg.enabled)
        .is_none()
    {
        return Err(AdminOpError::TelegramDisabled(tenant_id));
    }
    let bot_token_key = format!("tenants/{}/telegram/bot_token", tenant.id);
    let bot_token = secrets
        .get(&bot_token_key)
        .await
        .map_err(AdminOpError::Secret)?
        .ok_or_else(|| AdminOpError::MissingBotToken(tenant.id.clone()))?;
    api.delete_webhook(&bot_token, true)
        .await
        .map_err(AdminOpError::Telegram)?;
    tracing::info!(event = "telegram_admin_deregister", tenant = %tenant.id);
    Ok(DeregisterResponse {
        ok: true,
        tenant: tenant.id,
    })
}

async fn status_tenant_op(
    tenants: Vec<config::Tenant>,
    secrets: Arc<dyn SecretsManager>,
    api: Arc<dyn TelegramApi>,
    tenant_id: String,
) -> Result<StatusResponse, AdminOpError> {
    let tenant = tenants
        .into_iter()
        .find(|t| t.id == tenant_id)
        .ok_or_else(|| AdminOpError::TenantNotFound(tenant_id.clone()))?;
    let tenant_id = tenant.id.clone();
    let telegram_cfg = tenant
        .telegram
        .clone()
        .filter(|cfg| cfg.enabled)
        .ok_or_else(|| AdminOpError::TelegramDisabled(tenant_id.clone()))?;
    let bot_token_key = format!("tenants/{}/telegram/bot_token", tenant_id);
    let bot_token = secrets
        .get(&bot_token_key)
        .await
        .map_err(AdminOpError::Secret)?
        .ok_or_else(|| AdminOpError::MissingBotToken(tenant_id.clone()))?;
    let info = api
        .get_webhook_info(&bot_token)
        .await
        .map_err(AdminOpError::Telegram)?;
    let desired_url = desired_webhook_url(&telegram_cfg, &tenant_id);
    let matches = urls_match(&info.url, &desired_url);
    Ok(StatusResponse {
        ok: true,
        tenant: tenant_id,
        desired_url,
        matches,
        info,
    })
}

#[tokio::main]
async fn main() -> Result<()> {
    let telemetry = TelemetryConfig::from_env("gsm-ingress-telegram", env!("CARGO_PKG_VERSION"));
    init_telemetry(telemetry)?;

    let nats_url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
    let tenant = std::env::var("TENANT").unwrap_or_else(|_| "acme".into());
    let tenant_config_path = std::env::var("TENANT_CONFIG").ok();
    let tenants = load_tenants(tenant_config_path.as_deref(), &tenant)?;
    let secrets: Arc<dyn SecretsManager> = Arc::new(EnvSecretsManager::default());
    let api_base = std::env::var("TELEGRAM_API_BASE").ok();
    let http_client = Client::new();
    let telegram_impl = HttpTelegramApi::new(http_client, api_base);
    let telegram_api: Arc<dyn TelegramApi> = Arc::new(telegram_impl);
    let outcomes = reconcile_all_telegram_webhooks(&tenants, secrets.as_ref(), telegram_api.as_ref()).await;
    let secret_token = outcomes
        .iter()
        .find(|o| o.tenant == tenant)
        .and_then(|o| o.secret.clone())
        .or_else(|| std::env::var("TELEGRAM_SECRET_TOKEN").ok());
    let nats = async_nats::connect(nats_url).await?;
    let idem_guard = init_guard(&nats).await?;
    let dlq = DlqPublisher::new("ingress", nats.clone()).await?;
    let state = AppState {
        nats,
        tenant: tenant.clone(),
        secret_token,
        idem_guard,
        dlq,
        tenants: tenants.clone(),
        secrets: Arc::clone(&secrets),
        telegram_api: Arc::clone(&telegram_api),
    };

    let mut app = Router::new()
        .route("/telegram/webhook", post(handle_update))
        .route(
            "/admin/telegram/:tenant/register",
            post(admin_register),
        )
        .route(
            "/admin/telegram/:tenant/deregister",
            post(admin_deregister),
        )
        .route("/admin/telegram/:tenant/status", get(admin_status))
        .layer(rate_limit_layer(20, 10))
        .layer(middleware::from_fn(with_request_id))
        .layer(middleware::from_fn(verify_bearer))
        .layer(middleware::from_fn(verify_hmac));

    match ActionContext::from_env(&state.nats).await {
        Ok(ctx) => {
            let shared: SharedActionContext = std::sync::Arc::new(ctx);
            app = app
                .route("/a", get(handle_action).layer(Extension(shared.clone())))
                .route("/a/telegram", get(handle_action).layer(Extension(shared)));
        }
        Err(err) => {
            tracing::warn!(error = %err, "action links disabled for ingress-telegram");
        }
    }

    let app = app.with_state(state);

    let addr: std::net::SocketAddr = std::env::var("BIND")
        .unwrap_or_else(|_| "0.0.0.0:8080".into())
        .parse()
        .unwrap();
    tracing::info!("ingress-telegram listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn admin_register(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
) -> Result<Json<RegisterResponse>, (StatusCode, Json<ErrorResponse>)> {
    register_tenant_op(
        state.tenants.clone(),
        Arc::clone(&state.secrets),
        Arc::clone(&state.telegram_api),
        tenant_id,
    )
    .await
    .map(Json)
    .map_err(map_admin_error)
}



async fn admin_deregister(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
) -> Result<Json<DeregisterResponse>, (StatusCode, Json<ErrorResponse>)> {
    deregister_tenant_op(
        state.tenants.clone(),
        Arc::clone(&state.secrets),
        Arc::clone(&state.telegram_api),
        tenant_id,
    )
    .await
    .map(Json)
    .map_err(map_admin_error)
}

async fn admin_status(
    State(state): State<AppState>,
    Path(tenant_id): Path<String>,
) -> Result<Json<StatusResponse>, (StatusCode, Json<ErrorResponse>)> {
    status_tenant_op(
        state.tenants.clone(),
        Arc::clone(&state.secrets),
        Arc::clone(&state.telegram_api),
        tenant_id,
    )
    .await
    .map(Json)
    .map_err(map_admin_error)
}



#[derive(Debug, Deserialize, Serialize, Clone)]
struct TelegramUpdate {
    update_id: i64,
    #[serde(default)]
    message: Option<TelegramMessage>,
    #[serde(default)]
    edited_message: Option<TelegramMessage>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct TelegramMessage {
    message_id: i64,
    date: i64,
    #[serde(default)]
    text: Option<String>,
    chat: TelegramChat,
    from: Option<TelegramUser>,
    #[serde(default)]
    reply_to_message: Option<Box<ReplyMessageRef>>,
    #[serde(default)]
    message_thread_id: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct ReplyMessageRef {
    message_id: i64,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct TelegramChat {
    id: i64,
    #[serde(default)]
    r#type: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct TelegramUser {
    id: i64,
    username: Option<String>,
}

fn secret_token_valid(expected: &Option<String>, provided: Option<&str>) -> bool {
    match expected {
        Some(exp) => provided == Some(exp.as_str()),
        None => true,
    }
}

fn extract_message(update: &TelegramUpdate) -> Option<&TelegramMessage> {
    update.message.as_ref().or(update.edited_message.as_ref())
}

fn envelope_from_message(tenant: &str, msg: &TelegramMessage) -> MessageEnvelope {
    let chat_id = msg.chat.id.to_string();
    let user_id = msg
        .from
        .as_ref()
        .map(|u| u.id.to_string())
        .unwrap_or_else(|| "unknown".into());
    let thread_id = msg
        .reply_to_message
        .as_ref()
        .map(|reply| reply.message_id.to_string())
        .or_else(|| msg.message_thread_id.map(|id| id.to_string()));
    let ts = OffsetDateTime::from_unix_timestamp(msg.date as i64)
        .unwrap_or_else(|_| OffsetDateTime::now_utc())
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".into());

    MessageEnvelope {
        tenant: tenant.to_string(),
        platform: Platform::Telegram,
        chat_id: chat_id.clone(),
        user_id,
        thread_id,
        msg_id: format!("tg:{}", msg.message_id),
        text: msg.text.clone(),
        timestamp: ts,
        context: Default::default(),
    }
}


async fn handle_update(
    State(state): State<AppState>,
    request_id: Option<Extension<String>>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<Value>,
) -> axum::response::Response {
    let provided_token = headers
        .get("X-Telegram-Bot-Api-Secret-Token")
        .and_then(|v| v.to_str().ok());
    if !secret_token_valid(&state.secret_token, provided_token) {
        tracing::warn!("telegram secret token mismatch");
        return axum::http::StatusCode::UNAUTHORIZED.into_response();
    }

    let update: TelegramUpdate = match serde_json::from_value(payload.clone()) {
        Ok(u) => u,
        Err(e) => {
            tracing::warn!("bad update: {e}");
            return axum::http::StatusCode::BAD_REQUEST.into_response();
        }
    };

    if let Some(msg) = extract_message(&update).cloned() {
        let env = envelope_from_message(&state.tenant, &msg);
        let span = start_ingress_span(&env);
        let _guard = span.enter();
        let id_key = IdemKey {
            tenant: env.tenant.clone(),
            platform: env.platform.as_str().to_string(),
            msg_id: env.msg_id.clone(),
        };
        match state.idem_guard.should_process(&id_key).await {
            Ok(true) => {}
            Ok(false) => {
                record_idempotency_hit(&id_key.tenant);
                let rid_ref = request_id.as_ref().map(|Extension(id)| id);
                tracing::info!(
                    tenant = %id_key.tenant,
                    platform = %id_key.platform,
                    msg_id = %id_key.msg_id,
                    "duplicate telegram event dropped"
                );
                return ack202(rid_ref).into_response();
            }
            Err(err) => {
                tracing::error!(
                    error = %err,
                    tenant = %id_key.tenant,
                    platform = %id_key.platform,
                    msg_id = %id_key.msg_id,
                    "idempotency check failed; continuing"
                );
            }
        }
        let subject = in_subject(&state.tenant, env.platform.as_str(), &env.chat_id);
        if let Ok(bytes) = serde_json::to_vec(&env) {
            if let Err(e) = state.nats.publish(subject.clone(), bytes.into()).await {
                tracing::error!("publish failed: {e}");
                if let Err(dlq_err) = state
                    .dlq
                    .publish(
                        &state.tenant,
                        env.platform.as_str(),
                        &env.msg_id,
                        1,
                        DlqError {
                            code: "E_PUBLISH".into(),
                            message: e.to_string(),
                            stage: None,
                        },
                        &env,
                    )
                    .await
                {
                    tracing::error!("failed to publish dlq entry: {dlq_err}");
                }
                return axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response();
            } else {
                tracing::info!("published to {subject}");
                record_ingress(&env);
            }
        }

        let rid_ref = request_id.as_ref().map(|Extension(id)| id);
        return ack202(rid_ref).into_response();
    }

    axum::http::StatusCode::OK.into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use anyhow::anyhow;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    fn sample_message() -> TelegramMessage {
        TelegramMessage {
            message_id: 42,
            date: 1_700_000_000,
            text: Some("Hello".into()),
            chat: TelegramChat {
                id: 123,
                r#type: Some("private".into()),
            },
            from: Some(TelegramUser {
                id: 99,
                username: Some("bot".into()),
            }),
            reply_to_message: None,
            message_thread_id: None,
        }
    }

    #[test]
    fn extract_message_prefers_new_message() {
        let msg = sample_message();
        let update = TelegramUpdate {
            update_id: 1,
            message: Some(msg.clone()),
            edited_message: Some(msg),
        };
        let selected = extract_message(&update).unwrap();
        assert_eq!(selected.message_id, 42);
    }

    #[test]
    fn envelope_from_message_maps_fields() {
        let msg = sample_message();
        let env = envelope_from_message("tenant", &msg);
        assert_eq!(env.tenant, "tenant");
        assert_eq!(env.chat_id, "123");
        assert_eq!(env.user_id, "99");
        assert_eq!(env.msg_id, "tg:42");
        assert_eq!(env.text.as_deref(), Some("Hello"));
        assert!(env.thread_id.is_none());
    }

    #[test]
    fn envelope_includes_reply_to_message() {
        let mut msg = sample_message();
        msg.reply_to_message = Some(Box::new(ReplyMessageRef { message_id: 21 }));
        let env = envelope_from_message("tenant", &msg);
        assert_eq!(env.thread_id.as_deref(), Some("21"));
    }

    #[test]
    fn secret_token_validates_values() {
        let expected = Some("secret".to_string());
        assert!(secret_token_valid(&expected, Some("secret")));
        assert!(!secret_token_valid(&expected, Some("wrong")));
        assert!(!secret_token_valid(&expected, None));
        assert!(secret_token_valid(&None, None));
    }

    #[derive(Default)]
    struct MockSecrets {
        data: Mutex<HashMap<String, String>>,
        writable: bool,
    }

    impl MockSecrets {
        fn new(writable: bool) -> Self {
            Self {
                data: Mutex::new(HashMap::new()),
                writable,
            }
        }

        async fn insert(&self, key: &str, value: &str) {
            self.data
                .lock()
                .await
                .insert(key.to_string(), value.to_string());
        }
    }

    #[async_trait]
    impl SecretsManager for MockSecrets {
        async fn get(&self, key: &str) -> Result<Option<String>> {
            Ok(self.data.lock().await.get(key).cloned())
        }

        async fn put(&self, key: &str, value: &str) -> Result<()> {
            if self.writable {
                self.insert(key, value).await;
                Ok(())
            } else {
                Err(anyhow!("read-only"))
            }
        }

        fn can_write(&self) -> bool {
            self.writable
        }
    }

    #[derive(Default)]
    struct SetCall {
        url: String,
        allowed: Vec<String>,
        drop_pending: bool,
    }

    #[derive(Default)]
    struct MockTelegramApi {
        info: Mutex<WebhookInfo>,
        set_calls: Mutex<Vec<SetCall>>,
        delete_calls: Mutex<Vec<(String, bool)>>,
    }

    impl MockTelegramApi {
        fn new(info_url: &str) -> Self {
            Self {
                info: Mutex::new(WebhookInfo {
                    url: info_url.to_string(),
                    extra: Default::default(),
                }),
                set_calls: Default::default(),
                delete_calls: Default::default(),
            }
        }
    }

    #[async_trait]
    impl TelegramApi for MockTelegramApi {
        async fn get_webhook_info(&self, _bot_token: &str) -> Result<WebhookInfo> {
            Ok(self.info.lock().await.clone())
        }

        async fn set_webhook(
            &self,
            _bot_token: &str,
            url: &str,
            _secret: &str,
            allowed_updates: &[String],
            drop_pending: bool,
        ) -> Result<()> {
            self.set_calls.lock().await.push(SetCall {
                url: url.to_string(),
                allowed: allowed_updates.to_vec(),
                drop_pending,
            });
            Ok(())
        }

        async fn delete_webhook(&self, _bot_token: &str, drop_pending: bool) -> Result<()> {
            self.delete_calls
                .lock()
                .await
                .push((_bot_token.to_string(), drop_pending));
            Ok(())
        }
    }

    fn sample_tenant_config() -> config::Tenant {
        config::Tenant {
            id: "acme".into(),
            telegram: Some(config::TelegramConfig {
                enabled: true,
                public_webhook_base: "https://hook".into(),
                secret_token_key: "tenants/acme/telegram/secret_token".into(),
                allowed_updates: Some(vec!["message".into()]),
                drop_pending_on_first_install: Some(true),
            }),
        }
    }

    #[tokio::test]
    async fn register_op_invokes_set_webhook() {
        let tenants = vec![sample_tenant_config()];
        let secrets = Arc::new(MockSecrets::new(true));
        secrets
            .insert("tenants/acme/telegram/bot_token", "token-123")
            .await;
        secrets
            .insert("tenants/acme/telegram/secret_token", "secret-xyz")
            .await;
        let api = Arc::new(MockTelegramApi::new(""));

        let result = register_tenant_op(
            tenants,
            secrets.clone(),
            api.clone(),
            "acme".into(),
        )
        .await
        .expect("register should succeed");

        assert_eq!(result.tenant, "acme");
        assert_eq!(result.url, "https://hook/acme");
        let calls = api.set_calls.lock().await;
        assert_eq!(calls.len(), 1);
        let call = &calls[0];
        assert_eq!(call.url, "https://hook/acme");
        assert_eq!(call.allowed, vec!["message".to_string()]);
        assert!(!call.drop_pending);
    }

    #[tokio::test]
    async fn deregister_op_invokes_delete() {
        let tenants = vec![sample_tenant_config()];
        let secrets = Arc::new(MockSecrets::new(false));
        secrets
            .insert("tenants/acme/telegram/bot_token", "token-123")
            .await;
        let api = Arc::new(MockTelegramApi::new(""));

        let result = deregister_tenant_op(
            tenants,
            secrets.clone(),
            api.clone(),
            "acme".into(),
        )
        .await
        .expect("deregister should succeed");

        assert!(result.ok);
        let deletes = api.delete_calls.lock().await;
        assert_eq!(deletes.len(), 1);
        assert_eq!(deletes[0].1, true);
    }

    #[tokio::test]
    async fn status_op_reports_match() {
        let mut tenant = sample_tenant_config();
        tenant.telegram.as_mut().unwrap().public_webhook_base = "https://hook".into();
        let tenants = vec![tenant];
        let secrets = Arc::new(MockSecrets::new(false));
        secrets
            .insert("tenants/acme/telegram/bot_token", "token-123")
            .await;
        let api = Arc::new(MockTelegramApi::new("https://hook/acme"));

        let status = status_tenant_op(
            tenants,
            secrets.clone(),
            api.clone(),
            "acme".into(),
        )
        .await
        .expect("status should succeed");

        assert!(status.matches);
        assert_eq!(status.desired_url, "https://hook/acme");
    }
}
