#![cfg(feature = "e2e")]

use gsm_testutil::e2e::Harness;

#[test]
#[ignore]
fn dummy_e2e_smoke_test() {
    let harness = match Harness::new("slack") {
        Ok(h) => h,
        Err(err) => {
            eprintln!("skipping e2e dummy test: failed to initialise harness ({err})");
            return;
        }
    };

    if harness.config().is_none() && harness.resolver().is_none() {
        eprintln!("skipping e2e dummy test: secrets not configured");
        return;
    }

    let outbound = harness.outbound_text("C123", "ping");
    assert_eq!(outbound.text.as_deref(), Some("ping"));
    assert_eq!(harness.platform(), "slack");

    // Simple proof that the client is clonable and usable.
    let _client = harness.client();
}
