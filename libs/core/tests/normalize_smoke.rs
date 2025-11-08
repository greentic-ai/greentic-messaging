#![cfg(feature = "adaptive-cards")]

use std::fs;

use gsm_core::messaging_card::{MessageCardIr, ValidateError, normalizer};
use serde_json::Value;

#[test]
fn normalize_basic_fixture() -> Result<(), ValidateError> {
    let value = load_fixture("basic");
    let ir = normalizer::ac_to_ir(&value).expect("normalize");
    assert_eq!(ir.elements.len(), 1);
    Ok(())
}

#[test]
fn normalize_inputs_fixture_has_capabilities() {
    let value = load_fixture("inputs");
    let ir = normalizer::ac_to_ir(&value).expect("normalize");
    assert!(ir.meta.capabilities.contains("inputs"));
}

fn load_fixture(name: &str) -> Value {
    let path = format!("tests/fixtures/cards/{name}.json");
    let data = fs::read_to_string(path).expect("fixture missing");
    serde_json::from_str(&data).expect("invalid json")
}
