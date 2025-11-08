use std::sync::Arc;

use anyhow::Result;
use serde_json::Value;

pub mod adaptive;
pub mod downgrade;
pub mod ir;
pub mod renderers;
pub mod telemetry;
pub mod tier;
pub mod types;

pub use adaptive::{
    AdaptiveCardPayload, AdaptiveCardVersion, normalizer,
    validator::{ValidateError, validate_ac_json},
};
pub use downgrade::{CapabilityProfile, DowngradeContext, DowngradeEngine, PolicyDowngradeEngine};
pub use ir::{MessageCardIr, MessageCardIrBuilder};
pub use renderers::{
    NullRenderer, PlatformRenderer, RendererRegistry, SlackRenderer, TeamsRenderer,
    TelegramRenderer, WebChatRenderer, WebexRenderer, WhatsAppRenderer,
};
pub use telemetry::{CardTelemetry, NullTelemetry, TelemetryEvent, TelemetryHook};
pub use tier::Tier;
pub use types::{Action, ImageRef, MessageCard};

/// Entry point for migrating MessageCard payloads to the Adaptive pipeline.
pub struct MessageCardEngine {
    renderer_registry: RendererRegistry,
    downgrade: PolicyDowngradeEngine,
    telemetry: Arc<dyn TelemetryHook>,
}

impl Default for MessageCardEngine {
    fn default() -> Self {
        let mut registry = RendererRegistry::default();
        registry.register(TeamsRenderer::default());
        registry.register(WebChatRenderer::default());
        registry.register(SlackRenderer::default());
        registry.register(WebexRenderer::default());
        registry.register(TelegramRenderer::default());
        registry.register(WhatsAppRenderer::default());
        Self {
            renderer_registry: registry,
            downgrade: PolicyDowngradeEngine::default(),
            telemetry: Arc::new(NullTelemetry),
        }
    }
}

impl MessageCardEngine {
    pub fn new(renderer_registry: RendererRegistry) -> Self {
        Self {
            renderer_registry,
            downgrade: PolicyDowngradeEngine::default(),
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

    pub fn render(&self, platform: &str, ir: &MessageCardIr) -> Option<Value> {
        let renderer = self.renderer_registry.get(platform)?;
        let mut render_ir = if ir.tier > renderer.target_tier() {
            self.downgrade_for_platform(ir, platform, renderer.target_tier())
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
        let telemetry = CardTelemetry::new(self.telemetry.as_ref());
        telemetry.rendered(
            platform,
            render_ir.tier,
            render_ir.meta.warnings.len(),
            rendered.used_modal,
        );
        Some(rendered.payload)
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
}
