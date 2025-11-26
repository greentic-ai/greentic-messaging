use crate::context::TelemetryLabels;
use greentic_telemetry::metric;
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

pub fn record_counter(name: &'static str, value: u64, labels: &TelemetryLabels) {
    record_metric(name, value as f64, labels);
}

pub fn record_histogram(name: &'static str, value: f64, labels: &TelemetryLabels) {
    record_metric(name, value, labels);
}

pub fn record_gauge(name: &'static str, value: i64, labels: &TelemetryLabels) {
    record_metric(name, value as f64, labels);
}

fn record_metric(name: &'static str, value: f64, labels: &TelemetryLabels) {
    let storage = labels.tags();
    if storage.is_empty() {
        metric(name, value, &[]);
        return;
    }
    let attr_refs: Vec<(&str, &str)> = storage
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    metric(name, value, &attr_refs);
}
