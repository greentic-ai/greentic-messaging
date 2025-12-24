#![cfg(all(feature = "providers-bundle-tests", providers_bundle_present))]

mod provider_harness;

use provider_harness::{ProviderBackend, backend, implementation_origin, webchat_capabilities};

#[test]
fn gate_enforces_new_backend_when_requested() {
    if backend() == ProviderBackend::Legacy {
        eprintln!(
            "GREENTIC_TEST_PROVIDER_BACKEND=legacy (default); set GREENTIC_TEST_PROVIDER_BACKEND=new to exercise new providers."
        );
        return;
    }

    let caps = webchat_capabilities();
    assert_eq!(caps.metadata.provider_id, "webchat");
    assert!(
        implementation_origin().contains("greentic-messaging-providers"),
        "expected new providers origin marker"
    );
}
