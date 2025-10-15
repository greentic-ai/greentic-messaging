use once_cell::sync::Lazy;
use opentelemetry::global;
use opentelemetry::metrics::{Counter, Histogram, Meter, UpDownCounter};
use opentelemetry::KeyValue;
use std::collections::HashMap;
use std::sync::Mutex;

use crate::context::TelemetryLabels;
use crate::tracing_init::telemetry_enabled;

static COUNTERS: Lazy<Mutex<HashMap<&'static str, Counter<u64>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
static HISTOGRAMS: Lazy<Mutex<HashMap<&'static str, Histogram<f64>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
static GAUGES: Lazy<Mutex<HashMap<&'static str, UpDownCounter<i64>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

const METER_NAME: &str = super::tracing_init::TELEMETRY_METER_NAME;

fn meter() -> Meter {
    global::meter(METER_NAME)
}

pub fn record_counter(name: &'static str, value: u64, labels: &TelemetryLabels) {
    if !telemetry_enabled() {
        return;
    }
    let counter = {
        let mut guard = COUNTERS.lock().unwrap();
        guard
            .entry(name)
            .or_insert_with(|| meter().u64_counter(name).build())
            .clone()
    };
    let attrs = labels_to_kv(labels);
    counter.add(value, &attrs);
}

pub fn record_histogram(name: &'static str, value: f64, labels: &TelemetryLabels) {
    if !telemetry_enabled() {
        return;
    }
    let histogram = {
        let mut guard = HISTOGRAMS.lock().unwrap();
        guard
            .entry(name)
            .or_insert_with(|| meter().f64_histogram(name).build())
            .clone()
    };
    let attrs = labels_to_kv(labels);
    histogram.record(value, &attrs);
}

pub fn record_gauge(name: &'static str, value: i64, labels: &TelemetryLabels) {
    if !telemetry_enabled() {
        return;
    }
    let gauge = {
        let mut guard = GAUGES.lock().unwrap();
        guard
            .entry(name)
            .or_insert_with(|| meter().i64_up_down_counter(name).build())
            .clone()
    };
    let attrs = labels_to_kv(labels);
    gauge.add(value, &attrs);
}

fn labels_to_kv(labels: &TelemetryLabels) -> Vec<KeyValue> {
    labels
        .tags()
        .into_iter()
        .map(|(k, v)| KeyValue::new(k.to_string(), v))
        .collect()
}
