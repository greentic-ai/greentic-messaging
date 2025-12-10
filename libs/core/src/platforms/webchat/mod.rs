//! WebChat platform integration scaffolding.
//!
//! The detailed implementation will arrive in follow-up changes; this module
//! currently provides placeholders so other crates can start wiring against the
//! shared platform layout.

pub mod activity_bridge;
pub mod auth;
pub mod backoff;
pub mod bus;
pub mod circuit;
pub mod config;
pub mod conversation;
pub mod directline_client;
pub mod error;
pub mod http;
pub mod ingress;
pub mod mapper;
pub mod oauth;
pub mod provider;
pub mod session;
pub mod standalone;
pub mod telemetry;
pub mod transport;
pub mod types;

pub use activity_bridge::normalize_activity;
pub use auth::{Claims, TenantClaims, install_keys, sign, ttl, verify};
#[cfg(feature = "nats")]
pub use bus::NatsBus;
pub use bus::{EventBus, NoopBus, SharedBus, Subject};
pub use circuit::{CircuitBreaker, CircuitLabels, CircuitSettings};
pub use config::{Config, OAuthProviderConfig, SigningKeys};
#[cfg(feature = "store_sqlite")]
pub use conversation::sqlite_store;
pub use conversation::{
    Activity, ActivityPage, Attachment, ChannelAccount, ConversationAccount, ConversationStore,
    InMemoryConversationStore, MAX_ACTIVITY_HISTORY, SharedConversationStore, StoreError,
    StoredActivity, memory_store, noop_store,
};
pub use directline_client::{
    ConversationResponse, DirectLineApi, DirectLineError, MockDirectLineApi, ReqwestDirectLineApi,
    TokenResponse,
};
pub use error::WebChatError;
pub use ingress::{
    ActivitiesEnvelope, ActivitiesTransport, ActivitiesTransportResponse, Ingress, IngressCtx,
    IngressDeps, ReqwestActivitiesTransport, SharedActivitiesTransport, run_poll_loop,
};
pub use oauth::{
    CLOSE_WINDOW_HTML, CallbackQuery, GreenticOauthClient, OAuthRouteError,
    ReqwestGreenticOauthClient, StartQuery, contains_oauth_card,
};
pub use provider::{RouteContext, WebChatProvider};
pub use session::{MemorySessionStore, SharedSessionStore, WebchatSession, WebchatSessionStore};
#[cfg(feature = "directline_standalone")]
pub use standalone::{StandaloneState, router as standalone_router};
pub use types::{ConversationRef, GreenticEvent, IncomingMessage, MessagePayload, Participant};
