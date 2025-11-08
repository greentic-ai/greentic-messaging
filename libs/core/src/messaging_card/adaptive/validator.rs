use jsonschema::JSONSchema;
use once_cell::sync::Lazy;
use serde_json::Value;
use thiserror::Error;

pub static AC_16_SCHEMA: &str = include_str!("schema/ac-1.6.schema.json");

static COMPILED_SCHEMA: Lazy<JSONSchema> = Lazy::new(|| {
    let schema: Value =
        serde_json::from_str(AC_16_SCHEMA).expect("adaptive card schema must be valid JSON");
    JSONSchema::compile(&schema).expect("adaptive card schema must compile")
});

#[derive(Debug, Error)]
pub enum ValidateError {
    #[error("adaptive card payload must be an object")]
    NotObject,
    #[error("adaptive card validation failed: {0}")]
    Schema(String),
}

pub fn validate_ac_json(value: &Value) -> Result<(), ValidateError> {
    if !value.is_object() {
        return Err(ValidateError::NotObject);
    }

    COMPILED_SCHEMA
        .validate(value)
        .map(|_| ())
        .map_err(|errors| {
            let message = errors
                .map(|err| err.to_string())
                .collect::<Vec<_>>()
                .join("; ");
            ValidateError::Schema(message)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rejects_non_object_payloads() {
        let result = validate_ac_json(&json!(null));
        assert!(matches!(result, Err(ValidateError::NotObject)));
    }

    #[test]
    fn accepts_minimal_card() {
        let payload = json!({
            "$schema": "http://adaptivecards.io/schemas/adaptive-card.json",
            "type": "AdaptiveCard",
            "version": "1.6",
            "body": []
        });
        validate_ac_json(&payload).expect("valid card");
    }
}
