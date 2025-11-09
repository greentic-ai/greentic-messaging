use anyhow::Result;
use gsm_core::OutMessage;
use gsm_telemetry::{
    MessageContext, TelemetryLabels, record_counter, telemetry_enabled, with_common_fields,
};

const TRANSLATE_SPAN_NAME: &str = "translate.run";
const TRANSLATE_COUNTER: &str = "messages_translated";

pub fn translate_with_span<T, F>(out: &OutMessage, to_platform: &str, f: F) -> Result<T>
where
    F: FnOnce() -> Result<T>,
{
    let labels = TelemetryLabels {
        tenant: out.tenant.clone(),
        platform: Some(out.platform.as_str().to_string()),
        chat_id: Some(out.chat_id.clone()),
        msg_id: Some(out.message_id()),
        extra: Vec::new(),
    };
    let ctx = MessageContext::new(labels.clone());
    let from_platform = labels
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
        let mut labels = labels;
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
