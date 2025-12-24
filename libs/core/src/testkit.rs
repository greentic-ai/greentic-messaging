use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .to_path_buf()
}

/// Loads a JSON fixture relative to the workspace root.
pub fn load_fixture(path: impl AsRef<Path>) -> Value {
    let full = workspace_root().join(path.as_ref());
    let content = fs::read_to_string(&full)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", full.display()));
    serde_json::from_str(&content)
        .unwrap_or_else(|err| panic!("invalid json in {}: {err}", full.display()))
}

/// Normalizes two JSON values by sorting objects/arrays before comparing.
pub fn assert_json_eq_stable(left: &Value, right: &Value) {
    let normalized_left = normalize_json(left);
    let normalized_right = normalize_json(right);
    if normalized_left != normalized_right {
        panic!(
            "json mismatch\nleft:\n{}\nright:\n{}",
            serde_json::to_string_pretty(&normalized_left).unwrap_or_default(),
            serde_json::to_string_pretty(&normalized_right).unwrap_or_default()
        );
    }
}

#[cfg(feature = "adaptive-cards")]
pub fn canonical_card_snapshot(name: &str, card_value: Value) -> Value {
    let card = crate::adaptivecards::canonicalize(card_value)
        .unwrap_or_else(|err| panic!("canonicalize {name} failed: {err}"));
    crate::adaptivecards::stable_json(&card)
}

fn normalize_json(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            let mut out = serde_json::Map::new();
            for (k, v) in entries {
                out.insert(k.clone(), normalize_json(v));
            }
            Value::Object(out)
        }
        Value::Array(items) => {
            let mut normalized: Vec<Value> = items.iter().map(normalize_json).collect();
            normalized.sort_by(|a, b| {
                serde_json::to_string(a)
                    .unwrap_or_default()
                    .cmp(&serde_json::to_string(b).unwrap_or_default())
            });
            Value::Array(normalized)
        }
        other => other.clone(),
    }
}
