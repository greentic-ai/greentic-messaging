use serde_json::Value;

/// Returns true when the payload contains the provided text fragment anywhere in its structure.
pub fn message_contains_text(value: &Value, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }

    let mut stack = vec![value];
    while let Some(current) = stack.pop() {
        match current {
            Value::String(text) => {
                if text.contains(needle) {
                    return true;
                }
            }
            Value::Array(items) => {
                for item in items {
                    stack.push(item);
                }
            }
            Value::Object(map) => {
                for item in map.values() {
                    stack.push(item);
                }
            }
            _ => {}
        }
    }
    false
}

/// Asserts that a message contains a block with the specified type (e.g. Slack block type).
pub fn assert_has_block_type(value: &Value, block_type: &str) {
    assert!(
        has_block_type(value, block_type),
        "expected payload to contain block type `{}`, payload: {}",
        block_type,
        value
    );
}

/// Checks whether a block type is present anywhere inside the payload.
pub fn has_block_type(value: &Value, block_type: &str) -> bool {
    let mut stack = vec![value];
    while let Some(current) = stack.pop() {
        if let Some(blocks) = current.get("blocks").and_then(|b| b.as_array()) {
            if blocks.iter().any(|block| {
                block
                    .get("type")
                    .and_then(Value::as_str)
                    .map(|kind| kind.eq_ignore_ascii_case(block_type))
                    .unwrap_or(false)
            }) {
                return true;
            }
        }

        match current {
            Value::Array(items) => {
                for item in items {
                    stack.push(item);
                }
            }
            Value::Object(map) => {
                for item in map.values() {
                    stack.push(item);
                }
            }
            _ => {}
        }
    }
    false
}

/// Asserts that the first `"type"` entry in the payload matches the expected card type.
pub fn assert_card_type(value: &Value, expected: &str) {
    let card_type = find_card_type(value);
    assert_eq!(
        card_type.as_deref(),
        Some(expected),
        "expected card type `{}`, found `{:?}` in payload {}",
        expected,
        card_type,
        value
    );
}

/// Returns the first `"type"` field encountered while traversing the payload breadth-first.
pub fn find_card_type(value: &Value) -> Option<String> {
    let mut queue = vec![value];
    while let Some(current) = queue.pop() {
        if let Some(kind) = current.get("type").and_then(Value::as_str) {
            return Some(kind.to_string());
        }

        match current {
            Value::Array(items) => {
                for item in items {
                    queue.push(item);
                }
            }
            Value::Object(map) => {
                for item in map.values() {
                    queue.push(item);
                }
            }
            _ => {}
        }
    }
    None
}
