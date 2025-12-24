#![cfg(all(feature = "providers-bundle-tests", providers_bundle_present))]

mod provider_harness;

use provider_harness::{
    ProviderBackend, WebchatHarness, backend, load_vector_plan, vector_names, webchat_capabilities,
};

#[test]
fn webchat_vectors_use_new_providers_when_enabled() {
    if backend() == ProviderBackend::Legacy {
        eprintln!("GREENTIC_TEST_PROVIDER_BACKEND=legacy; skipping new provider vectors");
        return;
    }

    // ensure capabilities load before spinning up the component.
    let caps = webchat_capabilities();
    assert_eq!(caps.metadata.provider_id, "webchat");

    let mut harness = WebchatHarness::new();

    for name in vector_names() {
        let plan = load_vector_plan(name);
        let encoded = harness.encode(&plan);
        assert_eq!(
            encoded.content_type, "text/plain; charset=utf-8",
            "content type mismatch for vector {name}"
        );
        let expected_text = plan.summary_text.clone().unwrap_or_default();
        assert_eq!(
            encoded.body,
            serde_json::Value::String(expected_text),
            "body mismatch for vector {name}"
        );
        // warnings should be preserved in count/order.
        assert_eq!(encoded.warnings.len(), plan.warnings.len());
    }
}
