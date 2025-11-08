#![cfg(feature = "adaptive-cards")]

use std::fs;

use gsm_core::messaging_card::validate_ac_json;
use serde_json::Value;

#[test]
fn fixtures_validate_successfully() {
    for name in ["basic", "facts", "inputs", "showcard", "execute"] {
        let value = load_fixture(name);
        validate_ac_json(&value).expect("fixture must be valid");
    }
}

fn load_fixture(name: &str) -> Value {
    let path = format!("tests/fixtures/cards/{name}.json");
    let data = fs::read_to_string(path).expect("fixture missing");
    serde_json::from_str(&data).expect("invalid json")
}
