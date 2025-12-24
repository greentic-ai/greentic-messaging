#![cfg(all(feature = "providers-bundle-tests", providers_bundle_present))]

mod provider_harness;

use provider_harness::{ProviderBackend, backend, webchat_capabilities};

#[test]
fn providers_crate_links_and_exposes_capabilities() {
    if backend() == ProviderBackend::Legacy {
        eprintln!(
            "GREENTIC_TEST_PROVIDER_BACKEND not set to new; running probe against legacy default (capabilities still parsed from new providers)"
        );
    }

    let caps = webchat_capabilities();
    assert_eq!(caps.metadata.provider_id, "webchat");
    assert!(!caps.metadata.version.trim().is_empty());
    assert_eq!(caps.capabilities.supports_webhook_validation, true);
}
