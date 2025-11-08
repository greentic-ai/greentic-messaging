#![cfg(feature = "adaptive-cards")]

use gsm_core::messaging_card::validate_ac_json;
use serde_json::json;

#[test]
fn rejects_missing_body() {
    let payload = json!({
        "type": "AdaptiveCard",
        "version": "1.6"
    });

    let result = validate_ac_json(&payload);
    assert!(result.is_err());
}
