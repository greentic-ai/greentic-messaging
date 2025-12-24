use serde_json::{Map, Value};
use std::collections::BTreeMap;
use thiserror::Error;

/// Canonical representation of an Adaptive Card payload used for deterministic comparisons.
#[derive(Debug, Clone, PartialEq)]
pub struct CanonicalCard {
    pub version: String,
    pub content: Value,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CanonicalizeError {
    #[error("adaptive card payload must be an object")]
    NotObject,
    #[error("adaptive card missing required 'type' field")]
    MissingType,
    #[error("adaptive card type must be 'AdaptiveCard'")]
    InvalidType,
    #[error("adaptive card version missing or empty")]
    MissingVersion,
    #[error("adaptive card missing body array")]
    MissingBody,
    #[error("body must be an array when present")]
    BodyNotArray,
    #[error("actions must be an array when present")]
    ActionsNotArray,
    #[error("columns must be an array when present")]
    ColumnsNotArray,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ArrayKind {
    Generic,
    Body,
    Actions,
    Columns,
}

impl CanonicalCard {
    pub fn as_value(&self) -> Value {
        self.content.clone()
    }
}

pub fn canonicalize(value: Value) -> Result<CanonicalCard, CanonicalizeError> {
    let obj = value.as_object().ok_or(CanonicalizeError::NotObject)?;
    let type_field = obj
        .get("type")
        .and_then(|v| v.as_str())
        .ok_or(CanonicalizeError::MissingType)?;
    if !type_field.eq_ignore_ascii_case("adaptivecard") {
        return Err(CanonicalizeError::InvalidType);
    }
    let version = obj
        .get("version")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| "1.6".to_string());
    if version.trim().is_empty() {
        return Err(CanonicalizeError::MissingVersion);
    }

    let mut normalized = canonicalize_object(obj)?;
    // Enforce canonical casing and explicit version.
    normalized.insert("type".to_string(), Value::String("AdaptiveCard".into()));
    normalized.insert("version".to_string(), Value::String(version.clone()));

    Ok(CanonicalCard {
        version,
        content: Value::Object(normalized),
    })
}

pub fn stable_json(card: &CanonicalCard) -> Value {
    // Re-run canonicalization on the stored content to guarantee ordering stability even if the
    // caller mutated the value after canonicalize().
    match canonicalize_value(&card.content, ArrayKind::Generic) {
        Ok(value) => value,
        Err(_) => card.content.clone(),
    }
}

fn canonicalize_value(value: &Value, array_hint: ArrayKind) -> Result<Value, CanonicalizeError> {
    match value {
        Value::Object(map) => Ok(Value::Object(canonicalize_object(map)?)),
        Value::Array(items) => canonicalize_array(items, array_hint),
        other => {
            if array_hint == ArrayKind::Generic {
                Ok(other.clone())
            } else {
                Err(array_error(array_hint))
            }
        }
    }
}

fn canonicalize_object(map: &Map<String, Value>) -> Result<Map<String, Value>, CanonicalizeError> {
    let element_type = map.get("type").and_then(|v| v.as_str()).map(str::to_string);
    let mut out: BTreeMap<String, Value> = BTreeMap::new();

    for (key, value) in map {
        let hint = match key.as_str() {
            "body" => ArrayKind::Body,
            "actions" => ArrayKind::Actions,
            "columns" => ArrayKind::Columns,
            _ => ArrayKind::Generic,
        };
        let mut canonical = canonicalize_value(value, hint)?;
        if matches!(element_type.as_deref(), Some("TextBlock"))
            && key == "text"
            && let Some(text) = canonical.as_str()
        {
            canonical = Value::String(text.trim().to_string());
        }
        out.insert(key.clone(), canonical);
    }

    if matches!(element_type.as_deref(), Some("TextBlock")) && !out.contains_key("wrap") {
        out.insert("wrap".into(), Value::Bool(true));
    }

    Ok(out.into_iter().collect())
}

fn canonicalize_array(items: &[Value], kind: ArrayKind) -> Result<Value, CanonicalizeError> {
    let mut normalized: Vec<Value> = Vec::with_capacity(items.len());
    for value in items {
        normalized.push(canonicalize_value(value, ArrayKind::Generic)?);
    }

    if matches!(
        kind,
        ArrayKind::Body | ArrayKind::Actions | ArrayKind::Columns
    ) {
        normalized.sort_by_key(stable_value_key);
    }

    Ok(Value::Array(normalized))
}

fn stable_value_key(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let type_hint = map.get("type").and_then(|v| v.as_str()).unwrap_or_default();
            let label = map
                .get("id")
                .or_else(|| map.get("title"))
                .or_else(|| map.get("text"))
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let serialized = serde_json::to_string(value).unwrap_or_default();
            format!("{type_hint}:{label}:{serialized}")
        }
        other => other.to_string(),
    }
}

fn array_error(kind: ArrayKind) -> CanonicalizeError {
    match kind {
        ArrayKind::Body => CanonicalizeError::BodyNotArray,
        ArrayKind::Actions => CanonicalizeError::ActionsNotArray,
        ArrayKind::Columns => CanonicalizeError::ColumnsNotArray,
        ArrayKind::Generic => CanonicalizeError::NotObject,
    }
}
