use crate::messaging_card::ir::MessageCardIr;
use crate::messaging_card::tier::Tier;

use super::{PlatformRenderer, RenderMetrics, RenderOutput, adaptive_from_ir};

#[derive(Default)]
pub struct WebChatRenderer;

impl PlatformRenderer for WebChatRenderer {
    fn platform(&self) -> &'static str {
        "bf_webchat"
    }

    fn target_tier(&self) -> Tier {
        Tier::Premium
    }

    fn render(&self, ir: &MessageCardIr) -> RenderOutput {
        let mut warnings = Vec::new();
        let mut metrics = RenderMetrics::default();
        let payload = adaptive_from_ir(ir, &mut metrics, &mut warnings);
        let mut output = RenderOutput::new(payload);
        output.warnings = warnings;
        output.limit_exceeded = metrics.limit_exceeded;
        output.sanitized_count = metrics.sanitized_count;
        output.url_blocked_count = metrics.url_blocked_count;
        output
    }
}
