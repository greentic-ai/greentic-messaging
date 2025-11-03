use crate::context::TelemetryLabels;
use tracing::Span;

pub fn telemetry_enabled() -> bool {
    true
}

pub fn with_common_fields(span: &Span, tenant: &str, chat_id: Option<&str>, msg_id: Option<&str>) {
    span.record("tenant", tracing::field::display(tenant));
    if let Some(chat_id) = chat_id {
        span.record("chat_id", tracing::field::display(chat_id));
    }
    if let Some(msg_id) = msg_id {
        span.record("msg_id", tracing::field::display(msg_id));
    }
}

pub fn record_counter(_name: &'static str, _value: u64, _labels: &TelemetryLabels) {}

pub fn record_histogram(_name: &'static str, _value: f64, _labels: &TelemetryLabels) {}

pub fn record_gauge(_name: &'static str, _value: i64, _labels: &TelemetryLabels) {}
