use serde_json::Value;

use super::{CanonicalCard, CanonicalizeError};

/// Performs a lightweight structural validation on a canonical card.
///
/// This is intentionally cheaper than the full JSON Schema check and is aimed at catching
/// malformed envelopes before providers attempt translation.
pub fn validate(card: &CanonicalCard) -> Result<(), CanonicalizeError> {
    let Some(obj) = card.content.as_object() else {
        return Err(CanonicalizeError::NotObject);
    };

    match obj.get("type").and_then(|v| v.as_str()) {
        Some(kind) if kind.eq_ignore_ascii_case("adaptivecard") => {}
        Some(_) => return Err(CanonicalizeError::InvalidType),
        None => return Err(CanonicalizeError::MissingType),
    }

    if card.version.trim().is_empty() {
        return Err(CanonicalizeError::MissingVersion);
    }

    match obj.get("body") {
        Some(Value::Array(_)) => {}
        Some(_) => return Err(CanonicalizeError::BodyNotArray),
        None => return Err(CanonicalizeError::MissingBody),
    }

    if let Some(actions) = obj.get("actions")
        && !actions.is_array()
    {
        return Err(CanonicalizeError::ActionsNotArray);
    }

    if let Some(columns) = obj.get("columns")
        && !columns.is_array()
    {
        return Err(CanonicalizeError::ColumnsNotArray);
    }

    Ok(())
}
