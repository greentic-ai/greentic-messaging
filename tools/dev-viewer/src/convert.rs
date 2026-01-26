use serde::Serialize;
use serde_json::Value;

use gsm_core::messaging_card::ir::Meta;
use gsm_core::messaging_card::{RenderSnapshot, RenderSpec};
use gsm_core::{MessageCardEngine, Tier};

#[derive(Clone, Debug, Serialize)]
pub struct PlatformPreview {
    pub payload: Value,
    pub warnings: Vec<String>,
    pub tier: Tier,
    pub target_tier: Tier,
    pub downgraded: bool,
    pub used_modal: bool,
    pub limit_exceeded: bool,
    pub sanitized_count: usize,
    pub url_blocked_count: usize,
    pub meta: Meta,
}

impl PlatformPreview {
    fn from_snapshot(snapshot: RenderSnapshot) -> Self {
        let RenderSnapshot {
            output,
            ir,
            tier,
            target_tier,
            downgraded,
        } = snapshot;
        let (warnings, meta) = if let Some(ir) = ir {
            (ir.meta.warnings.clone(), ir.meta)
        } else {
            (output.warnings.clone(), Meta::default())
        };
        Self {
            payload: output.payload,
            warnings,
            tier,
            target_tier,
            downgraded,
            used_modal: output.used_modal,
            limit_exceeded: output.limit_exceeded,
            sanitized_count: output.sanitized_count,
            url_blocked_count: output.url_blocked_count,
            meta,
        }
    }
}

pub fn render_platform(
    engine: &MessageCardEngine,
    spec: &RenderSpec,
    platform: &str,
) -> Result<PlatformPreview, String> {
    let snapshot = engine
        .render_snapshot(platform, spec)
        .ok_or_else(|| format!("platform {platform} not supported by the renderer"))?;
    Ok(PlatformPreview::from_snapshot(snapshot))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gsm_core::messaging_card::{MessageCard, MessageCardKind};

    #[test]
    fn platform_preview_comes_from_engine_snapshot() {
        let engine = MessageCardEngine::bootstrap();
        let card = MessageCard {
            kind: MessageCardKind::Standard,
            title: Some("Test".into()),
            text: Some("Payload".into()),
            ..Default::default()
        };
        let spec = engine.render_spec(&card).expect("render spec");
        let preview = render_platform(&engine, &spec, "slack").expect("preview available");
        assert_eq!(preview.warnings.len(), 0);
        assert!(preview.payload.is_object());
    }
}
