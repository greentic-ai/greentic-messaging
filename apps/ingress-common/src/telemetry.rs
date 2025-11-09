use gsm_core::MessageEnvelope;
use gsm_telemetry::{MessageContext, TelemetryLabels, record_counter, with_common_fields};
use tracing::Span;

const INGRESS_COUNTER: &str = "messages_ingressed";
const IDEMPOTENCY_HIT_COUNTER: &str = "idempotency_hit";
const INGRESS_SPAN_NAME: &str = "ingress.handle";

/// Increment the ingress counter for a normalised message.
///
/// ```no_run
/// use gsm_core::{MessageEnvelope, Platform};
/// use gsm_ingress_common::record_ingress;
///
/// let env = MessageEnvelope {
///     tenant: "acme".into(),
///     platform: Platform::Webex,
///     chat_id: "room-1".into(),
///     user_id: "person-1".into(),
///     thread_id: None,
///     msg_id: "mid-1".into(),
///     text: Some("hello".into()),
///     timestamp: "2024-01-01T00:00:00Z".into(),
///     context: Default::default(),
/// };
/// record_ingress(&env);
/// ```
pub fn record_ingress(envelope: &MessageEnvelope) {
    let ctx = MessageContext::new(labels_from_envelope(envelope));
    record_counter(INGRESS_COUNTER, 1, &ctx.labels);
}

pub fn record_idempotency_hit(tenant: &str) {
    let labels = TelemetryLabels {
        tenant: tenant.to_string(),
        platform: None,
        chat_id: None,
        msg_id: None,
        extra: Vec::new(),
    };
    record_counter(IDEMPOTENCY_HIT_COUNTER, 1, &labels);
}

pub fn start_ingress_span(envelope: &MessageEnvelope) -> Span {
    let ctx = MessageContext::new(labels_from_envelope(envelope));
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

fn labels_from_envelope(env: &MessageEnvelope) -> TelemetryLabels {
    TelemetryLabels {
        tenant: env.tenant.clone(),
        platform: Some(env.platform.as_str().to_string()),
        chat_id: Some(env.chat_id.clone()),
        msg_id: Some(env.msg_id.clone()),
        extra: Vec::new(),
    }
}
