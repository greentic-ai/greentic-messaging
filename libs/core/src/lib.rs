//! Greentic Messaging core contracts and value types.
//!
//! This crate exposes the shared data structures exchanged between ingress, runner, and egress
//! components. It also provides validation helpers and small utilities for subject naming and
//! idempotency tracking.
pub mod adapter_registry;
pub mod cards;
#[cfg(feature = "component-host")]
pub mod component_host;
pub mod context;
pub mod default_packs;
pub mod egress;
pub mod http;
pub mod idempotency;
pub mod ingress;
pub mod interfaces;
#[cfg(feature = "adaptive-cards")]
pub mod messaging_card;
pub mod oauth;
pub mod outbound;
pub mod path_safety;
pub mod platforms;
pub mod prelude;
pub mod provider;
pub mod provider_capabilities;
pub mod provider_registry;
pub mod registry;
pub mod render_mode;
pub mod render_plan;
pub mod render_planner;
pub mod runner_client;
pub mod secrets_paths;
pub mod subjects;
pub mod telemetry;
pub mod types;
pub mod validate;
pub mod worker;

pub use adapter_registry::*;
pub use cards::*;
#[cfg(feature = "component-host")]
pub use component_host::*;
pub use context::*;
pub use default_packs::*;
pub use greentic_pack::messaging::MessagingAdapterKind;
pub use http::*;
pub use idempotency::*;
pub use ingress::*;
pub use interfaces::*;
#[cfg(feature = "adaptive-cards")]
pub use messaging_card::types::{
    Action as AdaptiveAction, ImageRef as AdaptiveImageRef, MessageCard as AdaptiveMessageCard,
    MessageCardKind as AdaptiveMessageCardKind, OauthCard as AdaptiveOauthCard,
    OauthPrompt as AdaptiveOauthPrompt, OauthProvider as AdaptiveOauthProvider,
};
#[cfg(feature = "adaptive-cards")]
pub use messaging_card::{
    MessageCardEngine,
    adaptive::{AdaptiveCardPayload, AdaptiveCardVersion, ValidateError, normalizer},
    downgrade::{CapabilityProfile, DowngradeContext, DowngradeEngine, PolicyDowngradeEngine},
    ir::{AppLink, Element, InputChoice, MessageCardIr, MessageCardIrBuilder},
    renderers::{
        NullRenderer, PlatformRenderer, RendererRegistry, SlackRenderer, TeamsRenderer,
        TelegramRenderer, WebChatRenderer, WebexRenderer,
    },
    spec::{AuthRenderSpec, FallbackButton, RenderIntent, RenderSpec},
    telemetry::{CardTelemetry, NullTelemetry, TelemetryEvent, TelemetryHook},
    tier::{Tier, TierPolicy},
};
pub use outbound::*;
pub use platforms::*;
pub use prelude::*;
pub use provider::*;
pub use provider_capabilities::*;
pub use provider_registry::{CapsSource, ProviderCapsRegistry, ProviderRecord};
pub use registry::*;
pub use render_mode::*;
pub use render_plan::*;
pub use render_planner::*;
pub use runner_client::*;
pub use secrets_paths::*;
pub use subjects::*;
pub use telemetry::*;
pub use types::*;
pub use validate::*;
pub use worker::*;

/// Returns the semantic version advertised by this crate.
///
/// ```
/// assert_eq!(gsm_core::version(), "0.1.0");
/// ```
pub fn version() -> &'static str {
    "0.1.0"
}
