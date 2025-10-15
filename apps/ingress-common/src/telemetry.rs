use gsm_core::MessageEnvelope;
use gsm_telemetry::{record_counter, with_common_fields, MessageContext};
use tracing::Span;

const INGRESS_COUNTER: &str = "messages_ingressed";
const INGRESS_SPAN_NAME: &str = "ingress.handle";

pub fn record_ingress(envelope: &MessageEnvelope) {
    let ctx = MessageContext::from_envelope(envelope);
    record_counter(INGRESS_COUNTER, 1, &ctx.labels);
}

pub fn start_ingress_span(envelope: &MessageEnvelope) -> Span {
    let ctx = MessageContext::from_envelope(envelope);
    let span = tracing::info_span!(
        INGRESS_SPAN_NAME,
        tenant = %ctx.labels.tenant,
        platform = %ctx.labels.platform.clone().unwrap_or_default(),
        chat_id = %ctx.labels.chat_id.clone().unwrap_or_default(),
        msg_id = %ctx.labels.msg_id.clone().unwrap_or_default()
    );
    with_common_fields(
        &span,
        &ctx.labels.tenant,
        ctx.labels.chat_id.as_deref(),
        ctx.labels.msg_id.as_deref(),
    );
    span
}
