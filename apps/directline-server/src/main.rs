use std::sync::Arc;

use anyhow::Result;
use axum::Router;
use greentic_secrets_spec::record_from_plain;
use gsm_core::platforms::webchat::{
    EventBus, GreenticEvent, IncomingMessage, MessagePayload, SharedBus, Subject, WebChatProvider,
    config::Config,
    conversation::{Activity, ChannelAccount, memory_store},
    session::MemorySessionStore,
    standalone::{StandaloneState, router as standalone_router},
};
use secrets_core::{
    ContentType, Scope, SecretListItem, SecretRecord, SecretUri, SecretVersion, SecretsBackend,
    VersionedSecret, Visibility,
};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tower_http::cors::{Any, CorsLayer};
use tracing::info;

/// Simple in-memory secrets backend that serves a single JWT signing key.
#[derive(Clone)]
struct StaticSecretsBackend {
    signing_key: String,
}

impl SecretsBackend for StaticSecretsBackend {
    fn put(&self, _record: SecretRecord) -> secrets_core::Result<SecretVersion> {
        Err(secrets_core::Error::Backend("read-only backend".into()))
    }

    fn get(
        &self,
        uri: &SecretUri,
        _version: Option<u64>,
    ) -> secrets_core::Result<Option<VersionedSecret>> {
        if uri.category() == "webchat" && uri.name() == "jwt_signing_key" {
            let record = record_from_plain(self.signing_key.clone());
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
    ) -> secrets_core::Result<Vec<SecretListItem>> {
        let uri = SecretUri::new(
            Scope::new("global", "webchat", None).expect("valid scope"),
            "webchat".to_string(),
            "jwt_signing_key".to_string(),
        )
        .expect("valid uri");
        Ok(vec![SecretListItem {
            uri,
            visibility: Visibility::Tenant,
            latest_version: Some("1".into()),
            content_type: ContentType::Opaque,
        }])
    }

    fn delete(&self, _uri: &SecretUri) -> secrets_core::Result<SecretVersion> {
        Err(secrets_core::Error::Backend("read-only backend".into()))
    }

    fn versions(&self, _uri: &SecretUri) -> secrets_core::Result<Vec<SecretVersion>> {
        Ok(vec![SecretVersion {
            version: 1,
            deleted: false,
        }])
    }

    fn exists(&self, uri: &SecretUri) -> secrets_core::Result<bool> {
        Ok(uri.category() == "webchat" && uri.name() == "jwt_signing_key")
    }
}

/// Echo bus that forwards incoming messages to a channel for auto-reply.
struct EchoBus {
    tx: mpsc::UnboundedSender<IncomingMessage>,
}

#[async_trait::async_trait]
impl EventBus for EchoBus {
    async fn publish(&self, _subject: &Subject, event: &GreenticEvent) -> Result<()> {
        let GreenticEvent::IncomingMessage(msg) = event;
        if matches!(&msg.payload, MessagePayload::Text { .. }) {
            let _ = self.tx.send(msg.clone());
        }
        Ok(())
    }
}

fn build_ac_reply(user_text: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "AdaptiveCard",
        "version": "1.3",
        "body": [
            {
                "type": "TextBlock",
                "text": "Greentic Bot",
                "weight": "Bolder",
                "size": "Medium"
            },
            {
                "type": "TextBlock",
                "text": format!("You said: \"{user_text}\""),
                "wrap": true
            },
            {
                "type": "TextBlock",
                "text": "This Adaptive Card is rendered natively via the Rust Direct Line server.",
                "wrap": true,
                "isSubtle": true
            },
            {
                "type": "FactSet",
                "facts": [
                    {"title": "Provider", "value": "WebChat"},
                    {"title": "AC Version", "value": "1.3"},
                    {"title": "Tier", "value": "A (Premium)"},
                    {"title": "Server", "value": "directline-server (Rust)"}
                ]
            }
        ],
        "actions": [
            {
                "type": "Action.OpenUrl",
                "title": "Greentic AI",
                "url": "https://greentic.ai"
            }
        ]
    })
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8080);

    let signing_key = std::env::var("JWT_SIGNING_KEY")
        .unwrap_or_else(|_| "greentic-demo-signing-key-2026".to_string());

    let base_url = format!("http://localhost:{port}/v3/directline");

    // Create secrets backend
    let backend = Arc::new(StaticSecretsBackend { signing_key });

    // Create WebChat provider
    let config = Config::with_base_url(&base_url);
    let scope = Scope::new("global", "webchat", None)?;
    let provider = WebChatProvider::new(config, backend).with_signing_scope(scope);

    // Create echo bus for auto-reply
    let (tx, mut rx) = mpsc::unbounded_channel::<IncomingMessage>();
    let bus: SharedBus = Arc::new(EchoBus { tx });

    // Create standalone state with echo bus
    let conversations = memory_store();
    let sessions = Arc::new(MemorySessionStore::default());
    let state =
        Arc::new(StandaloneState::with_store(provider, conversations, sessions, bus).await?);

    // Spawn echo bot background task
    let echo_state = Arc::clone(&state);
    tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            let user_text = match &msg.payload {
                MessagePayload::Text { text, .. } => text.clone(),
                _ => continue,
            };
            let conv_id = &msg.conversation.conversation_id;
            info!(conversation = %conv_id, text = %user_text, "echo bot replying");

            let ac = build_ac_reply(&user_text);
            let activity = Activity {
                id: String::new(),
                r#type: "message".into(),
                timestamp: None,
                from: Some(ChannelAccount {
                    id: "bot".into(),
                    name: Some("Greentic Bot".into()),
                    role: Some("bot".into()),
                }),
                recipient: None,
                conversation: None,
                text: Some(format!("Echo: {user_text}")),
                attachments: vec![gsm_core::platforms::webchat::conversation::Attachment {
                    content_type: "application/vnd.microsoft.card.adaptive".into(),
                    content: ac,
                    name: None,
                    thumbnail_url: None,
                }],
                channel_data: None,
                value: None,
                locale: None,
                reply_to_id: None,
                entities: vec![],
                service_url: None,
                channel_id: None,
                extra: Default::default(),
            };

            // Append bot activity to the conversation
            if let Err(err) = append_bot_activity(&echo_state, conv_id, activity).await {
                tracing::warn!(error = %err, "echo bot failed to reply");
            }
        }
    });

    // Build router with CORS
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new().merge(standalone_router(state)).layer(cors);

    let addr = format!("0.0.0.0:{port}");
    let listener = TcpListener::bind(&addr).await?;
    info!("Direct Line standalone server listening on http://{addr}");
    info!("Echo bot enabled - will reply to messages with Adaptive Cards");
    info!("Token endpoint: http://localhost:{port}/v3/directline/tokens/generate");
    info!("WebSocket: ws://localhost:{port}/v3/directline/conversations/{{id}}/stream");

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await?;

    Ok(())
}

/// Append a bot activity to a conversation and notify WebSocket subscribers.
async fn append_bot_activity(
    state: &StandaloneState,
    conversation_id: &str,
    mut activity: Activity,
) -> Result<()> {
    // Set bot defaults
    activity.conversation = Some(
        gsm_core::platforms::webchat::conversation::ConversationAccount {
            id: conversation_id.to_string(),
        },
    );
    if activity.id.is_empty() {
        activity.id = uuid::Uuid::new_v4().to_string();
    }
    activity.timestamp = Some(time::OffsetDateTime::now_utc());

    let stored = state
        .conversations
        .append(conversation_id, activity)
        .await
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;

    state
        .sessions
        .update_watermark(conversation_id, Some((stored.watermark + 1).to_string()))
        .await
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;

    Ok(())
}
