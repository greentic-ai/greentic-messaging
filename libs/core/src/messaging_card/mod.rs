use std::sync::Arc;

use anyhow::{Result, anyhow};
use serde_json::Value;

use crate::messaging_card::renderers::RenderOutput;

pub mod adaptive;
pub mod downgrade;
pub mod ir;
pub mod oauth_support;
pub mod renderers;
pub mod spec;
pub mod telemetry;
pub mod tier;
pub mod types;

pub use adaptive::{
    AdaptiveCardPayload, AdaptiveCardVersion, normalizer,
    validator::{ValidateError, validate_ac_json},
};
pub use downgrade::{CapabilityProfile, DowngradeContext, DowngradeEngine, PolicyDowngradeEngine};
pub use ir::{MessageCardIr, MessageCardIrBuilder};
pub use oauth_support::ensure_oauth_start_url;
pub use renderers::{
    NullRenderer, PlatformRenderer, RendererRegistry, SlackRenderer, TeamsRenderer,
    TelegramRenderer, WebChatRenderer, WebexRenderer, WhatsAppRenderer,
};
pub use spec::{AuthRenderSpec, FallbackButton, RenderIntent, RenderSpec};
pub use telemetry::{CardTelemetry, NullTelemetry, TelemetryEvent, TelemetryHook};
pub use tier::Tier;
pub use types::{
    Action, ImageRef, MessageCard, MessageCardKind, OauthCard, OauthPrompt, OauthProvider,
};

/// Entry point for migrating MessageCard payloads to the Adaptive pipeline.
pub struct MessageCardEngine {
    renderer_registry: RendererRegistry,
    downgrade: PolicyDowngradeEngine,
    telemetry: Arc<dyn TelemetryHook>,
}

impl Default for MessageCardEngine {
    fn default() -> Self {
        let mut registry = RendererRegistry::default();
        registry.register(TeamsRenderer);
        registry.register(WebChatRenderer);
        registry.register(SlackRenderer);
        registry.register(WebexRenderer);
        registry.register(TelegramRenderer);
        registry.register(WhatsAppRenderer);
        Self {
            renderer_registry: registry,
            downgrade: PolicyDowngradeEngine,
            telemetry: Arc::new(NullTelemetry),
        }
    }
}

impl MessageCardEngine {
    pub fn new(renderer_registry: RendererRegistry) -> Self {
        Self {
            renderer_registry,
            downgrade: PolicyDowngradeEngine,
            telemetry: Arc::new(NullTelemetry),
        }
    }

    /// Builds an engine with an empty renderer registry. Individual renderers are expected to be
    /// registered by the caller.
    pub fn bootstrap() -> Self {
        Self::default()
    }

    pub fn with_telemetry<T: TelemetryHook + 'static>(mut self, hook: T) -> Self {
        self.telemetry = Arc::new(hook);
        self
    }

    pub fn registry(&self) -> &RendererRegistry {
        &self.renderer_registry
    }

    pub fn register_renderer<R>(&mut self, renderer: R)
    where
        R: PlatformRenderer + 'static,
    {
        self.renderer_registry.register(renderer);
    }

    /// Converts user-authored MessageCards into the internal IR.
    pub fn normalize(&self, card: &MessageCard) -> Result<MessageCardIr> {
        if !matches!(card.kind, MessageCardKind::Standard) {
            return Err(anyhow!(
                "card kind {:?} requires render_spec() pipeline",
                card.kind
            ));
        }
        self.normalize_ir(card)
    }

    /// Produces a normalized render specification for downstream renderers.
    pub fn render_spec(&self, card: &MessageCard) -> Result<RenderSpec> {
        match card.kind {
            MessageCardKind::Standard => {
                let ir = self.normalize_ir(card)?;
                Ok(RenderSpec::Card(Box::new(ir)))
            }
            MessageCardKind::Oauth => {
                let oauth = card
                    .oauth
                    .as_ref()
                    .ok_or_else(|| anyhow!("oauth card missing oauth block"))?;
                Ok(RenderSpec::Auth(AuthRenderSpec::from_card(card, oauth)))
            }
        }
    }

    pub fn render(&self, platform: &str, ir: &MessageCardIr) -> Option<Value> {
        let snapshot = self.render_card_snapshot(platform, ir)?;
        self.record_render_event(
            platform,
            snapshot.tier,
            snapshot.warning_count(),
            &snapshot.output,
            snapshot.downgraded,
        );
        Some(snapshot.output.payload)
    }

    pub fn render_spec_payload(&self, platform: &str, spec: &RenderSpec) -> Option<Value> {
        self.render_snapshot_tracked(platform, spec)
            .map(|snapshot| snapshot.output.payload)
    }

    pub fn render_snapshot_tracked(
        &self,
        platform: &str,
        spec: &RenderSpec,
    ) -> Option<RenderSnapshot> {
        self.render_snapshot(platform, spec).map(|snapshot| {
            self.record_render_event(
                platform,
                snapshot.tier,
                snapshot.warning_count(),
                &snapshot.output,
                snapshot.downgraded,
            );
            snapshot
        })
    }

    pub fn render_snapshot(&self, platform: &str, spec: &RenderSpec) -> Option<RenderSnapshot> {
        match spec {
            RenderSpec::Card(ir) => self.render_card_snapshot(platform, ir.as_ref()),
            RenderSpec::Auth(auth) => self.render_auth_snapshot(platform, auth),
        }
    }

    pub fn render_card_snapshot(
        &self,
        platform: &str,
        ir: &MessageCardIr,
    ) -> Option<RenderSnapshot> {
        let renderer = self.renderer_registry.get(platform)?;
        let target_tier = renderer.target_tier();
        let downgraded = ir.tier > target_tier;
        let mut render_ir = if downgraded {
            self.downgrade_for_platform(ir, platform, target_tier)
        } else {
            ir.clone()
        };
        let rendered = renderer.render(&render_ir);
        if !rendered.warnings.is_empty() {
            render_ir
                .meta
                .warnings
                .extend(rendered.warnings.iter().cloned());
        }
        let tier = render_ir.tier;
        Some(RenderSnapshot {
            output: rendered,
            ir: Some(render_ir),
            tier,
            target_tier,
            downgraded,
        })
    }

    fn render_auth_snapshot(
        &self,
        platform: &str,
        auth: &AuthRenderSpec,
    ) -> Option<RenderSnapshot> {
        let renderer = self.renderer_registry.get(platform)?;
        if let Some(rendered) = renderer.render_auth(auth) {
            return Some(RenderSnapshot {
                output: rendered,
                ir: None,
                tier: Tier::Premium,
                target_tier: renderer.target_tier(),
                downgraded: false,
            });
        }

        let reason = if renderer.platform() == "teams" || renderer.platform() == "bf_webchat" {
            if auth.connection_name.is_none() {
                "missing connection name"
            } else {
                "native OAuth not supported"
            }
        } else {
            "native OAuth not supported"
        };

        let fallback_ir = self.oauth_fallback_ir(auth, platform, reason);
        self.render_card_snapshot(platform, &fallback_ir)
    }

    pub fn downgrade(&self, ir: &MessageCardIr, target_tier: Tier) -> MessageCardIr {
        let ctx = DowngradeContext::new(ir.tier, target_tier);
        self.downgrade_with_ctx(ir, ctx)
    }

    pub fn downgrade_for_platform(
        &self,
        ir: &MessageCardIr,
        platform: &str,
        target_tier: Tier,
    ) -> MessageCardIr {
        let ctx = DowngradeContext::new(ir.tier, target_tier).with_platform(platform);
        self.downgrade_with_ctx(ir, ctx)
    }

    fn downgrade_with_ctx(&self, ir: &MessageCardIr, ctx: DowngradeContext) -> MessageCardIr {
        if ir.tier <= ctx.target {
            return ir.clone();
        }

        let telemetry = CardTelemetry::new(self.telemetry.as_ref());
        telemetry.downgrading(ir.tier, ctx.target);
        self.downgrade.downgrade(ir, ctx)
    }
    fn record_render_event(
        &self,
        platform: &str,
        tier: Tier,
        warning_count: usize,
        rendered: &RenderOutput,
        downgraded: bool,
    ) {
        let telemetry = CardTelemetry::new(self.telemetry.as_ref());
        telemetry.rendered(
            platform,
            tier,
            warning_count,
            rendered.used_modal,
            rendered.limit_exceeded,
            rendered.sanitized_count,
            rendered.url_blocked_count,
            downgraded,
        );
    }

    fn oauth_fallback_ir(
        &self,
        auth: &AuthRenderSpec,
        platform: &str,
        reason: &str,
    ) -> MessageCardIr {
        let mut builder = MessageCardIrBuilder::default()
            .tier(Tier::Basic)
            .title(&auth.fallback_button.title);
        let description = format!("Sign in with {} to continue.", auth.provider.display_name());
        builder = builder.primary_text(&description, false);
        if let Some(url) = auth.fallback_button.url.as_deref() {
            builder = builder.open_url(&auth.fallback_button.title, url);
        }
        let mut ir = builder.build();
        ir.meta.source = Some("oauth-fallback".into());
        ir.meta
            .warn(format!("oauth card downgraded for {platform}: {reason}"));
        if auth.fallback_button.url.is_none() {
            ir.meta
                .warn("oauth fallback rendered without an action URL");
        }
        ir
    }
}

pub struct RenderSnapshot {
    pub output: RenderOutput,
    pub ir: Option<MessageCardIr>,
    pub tier: Tier,
    pub target_tier: Tier,
    pub downgraded: bool,
}

impl RenderSnapshot {
    pub fn warning_count(&self) -> usize {
        if let Some(ir) = &self.ir {
            ir.meta.warnings.len()
        } else {
            self.output.warnings.len()
        }
    }
}

impl MessageCardEngine {
    fn normalize_ir(&self, card: &MessageCard) -> Result<MessageCardIr> {
        #[cfg(feature = "adaptive-cards")]
        if let Some(ac) = &card.adaptive {
            validate_ac_json(ac)?;
            let mut ir = normalizer::ac_to_ir(ac)?;
            ir.auto_tier();
            ir.meta.source = Some("adaptive".into());
            ir.meta.adaptive_payload = Some(ac.clone());
            return Ok(ir);
        }

        let mut ir = MessageCardIr::from_plain(card);
        ir.meta.source = Some("plain".into());
        Ok(ir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn base_card() -> MessageCard {
        MessageCard {
            title: Some("Bootstrap".into()),
            text: Some("Hello".into()),
            ..Default::default()
        }
    }

    #[test]
    fn normalize_plain_card() {
        let engine = MessageCardEngine::bootstrap();
        let card = base_card();
        let ir = engine.normalize(&card).expect("normalization succeeds");
        assert_eq!(ir.head.title, Some("Bootstrap".into()));
        assert_eq!(ir.elements.len(), 1);
        assert_eq!(ir.tier, Tier::Basic);
        assert_eq!(ir.meta.source.as_deref(), Some("plain"));
    }

    #[test]
    fn normalize_adaptive_card() {
        let engine = MessageCardEngine::bootstrap();
        let mut card = base_card();
        card.adaptive = Some(json!({
            "type": "AdaptiveCard",
            "$schema": "http://adaptivecards.io/schemas/adaptive-card.json",
            "version": "1.6",
            "body": [
                {
                    "type": "TextBlock",
                    "text": "Adaptive hello"
                }
            ]
        }));

        let ir = engine
            .normalize(&card)
            .expect("adaptive normalization succeeds");
        assert_eq!(ir.elements.len(), 1);
        assert_eq!(ir.meta.source.as_deref(), Some("adaptive"));
    }

    #[test]
    fn downgrade_respects_target_tier() {
        let engine = MessageCardEngine::bootstrap();
        let card = base_card();
        let ir = engine.normalize(&card).unwrap();
        let downgraded = engine.downgrade(&ir, Tier::Basic);
        assert_eq!(downgraded.tier, Tier::Basic);
    }

    #[test]
    fn normalize_rejects_oauth_cards() {
        let engine = MessageCardEngine::bootstrap();
        let mut card = base_card();
        card.kind = MessageCardKind::Oauth;
        card.oauth = Some(OauthCard {
            provider: OauthProvider::Microsoft,
            scopes: vec!["User.Read".into()],
            resource: None,
            prompt: None,
            start_url: Some("https://oauth/start".into()),
            connection_name: Some("graph".into()),
            metadata: None,
        });
        let err = engine.normalize(&card).unwrap_err();
        assert!(
            err.to_string().contains("requires render_spec"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn render_spec_returns_auth_for_oauth_kind() {
        let engine = MessageCardEngine::bootstrap();
        let mut card = base_card();
        card.kind = MessageCardKind::Oauth;
        card.oauth = Some(OauthCard {
            provider: OauthProvider::Google,
            scopes: vec!["email".into()],
            resource: Some("https://www.googleapis.com/auth/userinfo.email".into()),
            prompt: Some(OauthPrompt::Consent),
            start_url: Some("https://oauth/google/start".into()),
            connection_name: Some("google-conn".into()),
            metadata: Some(json!({"tenant":"acme"})),
        });

        let spec = engine.render_spec(&card).expect("spec");
        assert!(matches!(spec.intent(), RenderIntent::Auth));
        let auth = spec.as_auth().expect("auth spec");
        assert_eq!(auth.provider, OauthProvider::Google);
        assert_eq!(auth.connection_name.as_deref(), Some("google-conn"));
        assert_eq!(
            auth.start_url.as_deref(),
            Some("https://oauth/google/start")
        );
        assert_eq!(auth.fallback_button.title, "Bootstrap");
        assert_eq!(
            auth.metadata
                .as_ref()
                .and_then(|m| m.get("tenant").and_then(|v| v.as_str())),
            Some("acme")
        );
    }
}
