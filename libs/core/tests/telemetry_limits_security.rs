#![cfg(feature = "adaptive-cards")]

use std::fs;
use std::sync::{Arc, Mutex, MutexGuard};

use gsm_core::messaging_card::renderers::{
    override_url_allow_list, reload_url_allow_list_from_env,
};
use gsm_core::messaging_card::{MessageCard, MessageCardEngine, TelemetryEvent, TelemetryHook};
use once_cell::sync::Lazy;
use serde_json::Value;

#[test]
fn url_allow_list_blocks_disallowed_links() {
    let _guard = AllowListGuard::new(Some(vec!["https://allowed.example/".into()]));
    let (hook, events) = recording_hook();
    let engine = MessageCardEngine::bootstrap().with_telemetry(hook);
    let card = load_card("markdown_and_links.json");
    let ir = engine.normalize(&card).expect("normalize card");
    let payload = engine.render("slack", &ir).expect("render slack payload");

    let blocks = payload["blocks"]
        .as_array()
        .expect("blocks array on slack payload");
    let actions = blocks
        .iter()
        .find(|block| block.get("type") == Some(&Value::String("actions".into())))
        .expect("actions block present");
    let elements = actions["elements"]
        .as_array()
        .expect("actions array on slack payload");
    assert_eq!(
        elements.len(),
        1,
        "blocked URLs are removed from the rendered payload"
    );

    let event = last_event(&events);
    match event {
        TelemetryEvent::Rendered {
            platform,
            url_blocked_count,
            downgrade_count,
            native_count,
            ..
        } => {
            assert_eq!(platform, "slack");
            assert_eq!(url_blocked_count, 1);
            assert_eq!(downgrade_count, 0);
            assert_eq!(native_count, 1);
        }
        other => panic!("expected rendered telemetry, got {other:?}"),
    }
}

#[test]
fn sanitizer_strips_tags_and_records_metrics() {
    let _guard = AllowListGuard::new(None);
    let (hook, events) = recording_hook();
    let engine = MessageCardEngine::bootstrap().with_telemetry(hook);
    let card = load_card("markdown_and_links.json");
    let ir = engine.normalize(&card).expect("normalize card");
    let payload = engine.render("slack", &ir).expect("render slack payload");

    let blocks = payload["blocks"].as_array().expect("slack blocks array");
    let header = blocks
        .iter()
        .find(|block| block.get("type") == Some(&Value::String("header".into())))
        .expect("header block");
    let header_text = header["text"]["text"].as_str().expect("header text");
    assert!(
        !header_text.contains('<'),
        "sanitizer removes html tags from header"
    );
    assert!(
        header_text.contains("Greeter"),
        "header retains plain text content"
    );

    let footer = blocks
        .iter()
        .find(|block| block.get("type") == Some(&Value::String("context".into())))
        .expect("footer context block");
    let footer_text = footer["elements"][0]["text"].as_str().expect("footer text");
    assert!(
        !footer_text.contains('<'),
        "sanitizer removes html tags from footer"
    );

    let event = last_event(&events);
    match event {
        TelemetryEvent::Rendered {
            sanitized_count,
            downgrade_count,
            native_count,
            ..
        } => {
            assert!(
                sanitized_count >= 1,
                "sanitizer increments telemetry counter"
            );
            assert_eq!(downgrade_count, 0);
            assert_eq!(native_count, 1);
        }
        other => panic!("expected rendered telemetry, got {other:?}"),
    }
}

#[test]
fn payload_limit_sets_flag_and_warning() {
    let _guard = AllowListGuard::new(None);
    let (hook, events) = recording_hook();
    let engine = MessageCardEngine::bootstrap().with_telemetry(hook);
    let mut card = load_card("payload_overflow.json");
    if let Some(text) = card.text.as_mut() {
        *text = text.repeat(1500);
    }
    let ir = engine.normalize(&card).expect("normalize card");
    let payload = engine
        .render("telegram", &ir)
        .expect("render telegram payload");

    let truncated = payload["text"].as_str().expect("telegram text");
    assert!(
        truncated.chars().count() < card.text.as_ref().unwrap().chars().count(),
        "payload is truncated to platform limit"
    );

    let event = last_event(&events);
    match event {
        TelemetryEvent::Rendered {
            limit_exceeded,
            warnings,
            downgrade_count,
            native_count,
            ..
        } => {
            assert!(limit_exceeded, "Telemetry marks limit overflow");
            assert!(
                warnings > 0,
                "Telemetry captures warning count for truncation"
            );
            assert_eq!(downgrade_count, 0);
            assert_eq!(native_count, 1);
        }
        other => panic!("expected rendered telemetry, got {other:?}"),
    }
}

fn load_card(name: &str) -> MessageCard {
    let path = format!("tests/fixtures/security/{name}");
    let data = fs::read_to_string(path).expect("fixture missing");
    serde_json::from_str(&data).expect("invalid MessageCard fixture")
}

fn last_event(events: &Arc<Mutex<Vec<TelemetryEvent>>>) -> TelemetryEvent {
    events
        .lock()
        .expect("telemetry mutex poisoned")
        .last()
        .cloned()
        .expect("expected telemetry event")
}

fn recording_hook() -> (RecordingHook, Arc<Mutex<Vec<TelemetryEvent>>>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    (
        RecordingHook {
            events: events.clone(),
        },
        events,
    )
}

#[derive(Clone)]
struct RecordingHook {
    events: Arc<Mutex<Vec<TelemetryEvent>>>,
}

impl TelemetryHook for RecordingHook {
    fn emit(&self, event: TelemetryEvent) {
        self.events
            .lock()
            .expect("telemetry mutex poisoned")
            .push(event);
    }
}

static ALLOW_LIST_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

struct AllowListGuard {
    _guard: MutexGuard<'static, ()>,
}

impl AllowListGuard {
    fn new(list: Option<Vec<String>>) -> Self {
        let guard = ALLOW_LIST_LOCK
            .lock()
            .expect("allow list test mutex poisoned");
        override_url_allow_list(list);
        Self { _guard: guard }
    }
}

impl Drop for AllowListGuard {
    fn drop(&mut self) {
        reload_url_allow_list_from_env();
    }
}
