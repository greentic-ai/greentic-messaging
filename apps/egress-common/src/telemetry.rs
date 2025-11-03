use gsm_core::OutMessage;
use gsm_telemetry::{
    MessageContext, record_counter, record_histogram, set_current_tenant_ctx, with_common_fields,
};
use tracing::Span;

const EGRESS_ACQUIRE_SPAN: &str = "egress.acquire_permit";
const EGRESS_SEND_SPAN: &str = "egress.send";
const EGRESS_COUNTER: &str = "messages_egressed";
const EGRESS_LATENCY_HISTOGRAM: &str = "histogram.egress_latency_ms";

pub fn context_from_out(out: &OutMessage) -> MessageContext {
    set_current_tenant_ctx(out.ctx.clone());
    MessageContext::from_out(out)
}

pub fn start_acquire_span(ctx: &MessageContext) -> Span {
    let platform = ctx
        .labels
        .platform
        .clone()
        .unwrap_or_else(|| "unknown".to_string());
    let span = tracing::info_span!(
        EGRESS_ACQUIRE_SPAN,
        tenant = tracing::field::Empty,
        platform = %platform,
        chat_id = tracing::field::Empty,
        msg_id = tracing::field::Empty
    );
    with_common_fields(
        &span,
        &ctx.labels.tenant,
        ctx.labels.chat_id.as_deref(),
        ctx.labels.msg_id.as_deref(),
    );
    span
}

pub fn start_send_span(ctx: &MessageContext) -> Span {
    let platform = ctx
        .labels
        .platform
        .clone()
        .unwrap_or_else(|| "unknown".to_string());
    let span = tracing::info_span!(
        EGRESS_SEND_SPAN,
        tenant = tracing::field::Empty,
        platform = %platform,
        chat_id = tracing::field::Empty,
        msg_id = tracing::field::Empty
    );
    with_common_fields(
        &span,
        &ctx.labels.tenant,
        ctx.labels.chat_id.as_deref(),
        ctx.labels.msg_id.as_deref(),
    );
    span
}

pub fn record_egress_success(ctx: &MessageContext, latency_ms: f64) {
    record_latency(ctx, latency_ms);
    record_counter(EGRESS_COUNTER, 1, &ctx.labels);
}

pub fn record_latency(ctx: &MessageContext, latency_ms: f64) {
    record_histogram(EGRESS_LATENCY_HISTOGRAM, latency_ms, &ctx.labels);
}
