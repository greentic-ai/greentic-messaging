#![cfg_attr(docsrs, feature(doc_auto_cfg))]
#![forbid(unsafe_code)]

#[cfg(feature = "webchat_bf_mode")]
pub mod activity_bridge;
#[cfg(feature = "webchat_bf_mode")]
pub mod auth;
#[cfg(feature = "webchat_bf_mode")]
mod backoff;
#[cfg(feature = "webchat_bf_mode")]
pub mod bus;
#[cfg(feature = "webchat_bf_mode")]
pub mod circuit;
#[cfg(feature = "webchat_bf_mode")]
pub mod config;
#[cfg(feature = "webchat_bf_mode")]
pub mod directline_client;
#[cfg(feature = "webchat_bf_mode")]
pub mod error;
#[cfg(feature = "webchat_bf_mode")]
pub mod http;
#[cfg(feature = "webchat_bf_mode")]
pub mod ingress;
#[cfg(feature = "webchat_bf_mode")]
pub mod oauth;
#[cfg(feature = "webchat_bf_mode")]
pub mod session;
#[cfg(feature = "webchat_bf_mode")]
pub mod telemetry;
#[cfg(feature = "webchat_bf_mode")]
pub mod types;

#[cfg(feature = "webchat_bf_mode")]
pub use http::{AppState, router};

#[cfg(feature = "directline_standalone")]
pub mod conversation;
#[cfg(feature = "directline_standalone")]
pub mod jwt;
#[cfg(feature = "directline_standalone")]
pub mod standalone;
#[cfg(feature = "directline_standalone")]
pub use standalone::{StandaloneState, router as standalone_router};

#[cfg(feature = "store_sqlite")]
pub use conversation::sqlite_store;
#[cfg(feature = "store_redis")]
pub use conversation::redis_store;
