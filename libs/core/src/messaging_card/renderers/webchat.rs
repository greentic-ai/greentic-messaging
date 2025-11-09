use crate::messaging_card::ir::MessageCardIr;
use crate::messaging_card::spec::AuthRenderSpec;
use crate::messaging_card::tier::Tier;

use super::{PlatformRenderer, RenderMetrics, RenderOutput, adaptive_from_ir};
use serde_json::json;

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

    fn render_auth(&self, auth: &AuthRenderSpec) -> Option<RenderOutput> {
        let connection = auth.connection_name.as_deref()?;
        let button_title = auth.fallback_button.title.clone();
        let mut payload = json!({
            "type": "message",
            "attachments": [{
                "contentType": "application/vnd.microsoft.card.oauth",
                "content": {
                    "text": format!("Sign in with {}", auth.provider.display_name()),
                    "connectionName": connection,
                    "buttons": [{
                        "type": "signin",
                        "title": button_title,
                    }]
                }
            }]
        });

        if let Some(url) = auth
            .start_url
            .as_ref()
            .or(auth.fallback_button.url.as_ref())
        {
            payload["attachments"][0]["content"]["buttons"][0]["value"] = json!(url);
        }

        Some(RenderOutput::new(payload))
    }
}
