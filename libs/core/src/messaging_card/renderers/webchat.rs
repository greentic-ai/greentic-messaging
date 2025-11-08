use crate::messaging_card::ir::MessageCardIr;
use crate::messaging_card::tier::Tier;

use super::{PlatformRenderer, RenderOutput, adaptive_from_ir};

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
        RenderOutput::new(adaptive_from_ir(ir))
    }
}
