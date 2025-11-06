#![cfg_attr(docsrs, feature(doc_auto_cfg))]
#![forbid(unsafe_code)]

#[cfg(feature = "webchat_bf_mode")]
pub use gsm_core::platforms::webchat::activity_bridge;
#[cfg(feature = "webchat_bf_mode")]
pub use gsm_core::platforms::webchat::auth;
#[cfg(feature = "webchat_bf_mode")]
#[allow(unused_imports)]
pub(crate) use gsm_core::platforms::webchat::backoff;
#[cfg(feature = "webchat_bf_mode")]
pub use gsm_core::platforms::webchat::bus;
#[cfg(feature = "webchat_bf_mode")]
pub use gsm_core::platforms::webchat::circuit;
#[cfg(feature = "webchat_bf_mode")]
pub use gsm_core::platforms::webchat::directline_client;
#[cfg(feature = "webchat_bf_mode")]
pub use gsm_core::platforms::webchat::error;
#[cfg(feature = "webchat_bf_mode")]
pub use gsm_core::platforms::webchat::http;
#[cfg(feature = "webchat_bf_mode")]
pub use gsm_core::platforms::webchat::ingress;
#[cfg(feature = "webchat_bf_mode")]
pub use gsm_core::platforms::webchat::oauth;
#[cfg(feature = "webchat_bf_mode")]
pub use gsm_core::platforms::webchat::session;
#[cfg(feature = "webchat_bf_mode")]
pub use gsm_core::platforms::webchat::telemetry;
#[cfg(feature = "webchat_bf_mode")]
pub use gsm_core::platforms::webchat::types;

pub use gsm_core::platforms::webchat::config;
pub use gsm_core::platforms::webchat::{Config, OAuthProviderConfig, SigningKeys};

#[cfg(feature = "webchat_bf_mode")]
pub use http::{AppState, router};

#[cfg(feature = "directline_standalone")]
pub use gsm_core::platforms::webchat::auth as jwt;
#[cfg(feature = "directline_standalone")]
pub use gsm_core::platforms::webchat::conversation;
#[cfg(feature = "directline_standalone")]
pub use gsm_core::platforms::webchat::standalone;
#[cfg(feature = "directline_standalone")]
pub use standalone::{StandaloneState, router as standalone_router};

#[cfg(feature = "store_redis")]
pub use conversation::redis_store;
#[cfg(feature = "store_sqlite")]
pub use conversation::sqlite_store;

pub use gsm_core::platforms::webchat::{RouteContext, WebChatProvider};
