use anyhow::Result;
use gsm_core::OutMessage;
use gsm_telemetry::{record_counter, telemetry_enabled, with_common_fields, MessageContext};

const TRANSLATE_SPAN_NAME: &str = "translate.run";
const TRANSLATE_COUNTER: &str = "messages_translated";

pub fn translate_with_span<T, F>(out: &OutMessage, to_platform: &str, f: F) -> Result<T>
where
    F: FnOnce() -> Result<T>,
{
    let ctx = MessageContext::from_out(out);
    let from_platform = ctx
        .labels
        .platform
        .clone()
        .unwrap_or_else(|| "unknown".to_string());
    let span = tracing::info_span!(
        TRANSLATE_SPAN_NAME,
        tenant = tracing::field::Empty,
        chat_id = tracing::field::Empty,
        msg_id = tracing::field::Empty,
        from_platform = %from_platform,
        to_platform = %to_platform
    );
    with_common_fields(
        &span,
        &ctx.labels.tenant,
        ctx.labels.chat_id.as_deref(),
        ctx.labels.msg_id.as_deref(),
    );
    let _guard = span.enter();
    let result = f();
    if result.is_ok() && telemetry_enabled() {
        let mut labels = ctx.labels.clone();
        labels.platform = None;
        labels.chat_id = None;
        labels.msg_id = None;
        labels
            .extra
            .push(("from_platform".into(), from_platform.clone()));
        labels
            .extra
            .push(("to_platform".into(), to_platform.to_string()));
        record_counter(TRANSLATE_COUNTER, 1, &labels);
    }
    result
}
