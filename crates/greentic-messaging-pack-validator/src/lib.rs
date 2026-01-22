#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

use greentic_types::{PROVIDER_EXTENSION_ID, PackManifest, ProviderDecl, decode_pack_manifest};

wit_bindgen::generate!({
    world: "pack-validator",
    path: "wit/greentic/pack-validate@0.1.0",
});

use exports::greentic::pack_validate::validator::{Diagnostic, Guest, PackInputs};

struct MessagingPackValidator;

impl Guest for MessagingPackValidator {
    fn applies(inputs: PackInputs) -> bool {
        decode_manifest(&inputs.manifest_cbor)
            .as_ref()
            .map(is_messaging_pack)
            .unwrap_or(false)
    }

    fn validate(inputs: PackInputs) -> Vec<Diagnostic> {
        let Some(manifest) = decode_manifest(&inputs.manifest_cbor) else {
            return Vec::new();
        };
        if !is_messaging_pack(&manifest) {
            return Vec::new();
        }

        let mut diagnostics = Vec::new();
        diagnostics.extend(validate_provider_decls(&manifest));
        if has_setup_declaration(&manifest) {
            diagnostics.extend(validate_setup_flow(&manifest));
        }
        if has_subscriptions_declaration(&manifest) {
            diagnostics.extend(validate_subscriptions_flow(&manifest));
        }
        diagnostics.extend(validate_secret_requirements(&manifest));
        diagnostics
    }
}

#[cfg(target_arch = "wasm32")]
export!(MessagingPackValidator);

fn decode_manifest(bytes: &[u8]) -> Option<PackManifest> {
    decode_pack_manifest(bytes).ok()
}

fn provider_decls(manifest: &PackManifest) -> Vec<&ProviderDecl> {
    manifest
        .provider_extension_inline()
        .map(|inline| inline.providers.iter().collect())
        .unwrap_or_default()
}

fn is_messaging_pack(manifest: &PackManifest) -> bool {
    if manifest.pack_id.as_str().starts_with("messaging-") {
        return true;
    }

    if let Some(inline) = manifest.provider_extension_inline() {
        for provider in &inline.providers {
            let provider_type = provider.provider_type.trim();
            let provider_type_lower = provider_type.to_ascii_lowercase();
            if provider_type_lower.starts_with("messaging.")
                || provider_type_lower.starts_with("greentic:provider/messaging.")
                || (provider_type_lower.starts_with("greentic:provider/")
                    && provider_type_lower.contains("messaging."))
            {
                return true;
            }
            if provider
                .config_schema_ref
                .to_ascii_lowercase()
                .contains("schemas/messaging/")
            {
                return true;
            }
        }
    }

    false
}

fn flow_id_is_setup_like(flow_id: &str) -> bool {
    let flow_id = flow_id.to_ascii_lowercase();
    flow_id == "setup"
        || flow_id.starts_with("setup_")
        || flow_id.starts_with("setup-")
        || flow_id.starts_with("setup.")
}

fn flow_id_mentions_subscription(flow_id: &str) -> bool {
    flow_id.to_ascii_lowercase().contains("subscription")
}

fn flow_id_is_subscriptions_flow(flow_id: &str) -> bool {
    let flow_id = flow_id.to_ascii_lowercase();
    flow_id.contains("sync-subscriptions") || flow_id.contains("subscriptions")
}

fn flow_has_entrypoint(flow: &greentic_types::PackFlowEntry, entrypoint: &str) -> bool {
    flow.entrypoints
        .iter()
        .any(|entry| entry.eq_ignore_ascii_case(entrypoint))
        || flow
            .flow
            .entrypoints
            .keys()
            .any(|entry| entry.eq_ignore_ascii_case(entrypoint))
}

fn has_setup_declaration(manifest: &PackManifest) -> bool {
    manifest
        .flows
        .iter()
        .any(|flow| flow_id_is_setup_like(flow.id.as_str()) || flow_has_entrypoint(flow, "setup"))
}

fn has_setup_entry(manifest: &PackManifest) -> bool {
    manifest
        .flows
        .iter()
        .any(|flow| flow_has_entrypoint(flow, "setup"))
}

fn has_subscriptions_declaration(manifest: &PackManifest) -> bool {
    manifest.flows.iter().any(|flow| {
        flow_id_mentions_subscription(flow.id.as_str())
            || flow_has_entrypoint(flow, "subscriptions")
    })
}

fn has_subscriptions_flow(manifest: &PackManifest) -> bool {
    manifest.flows.iter().any(|flow| {
        flow_id_is_subscriptions_flow(flow.id.as_str())
            || flow_has_entrypoint(flow, "subscriptions")
    })
}

fn provider_mentions_public_url(provider: &ProviderDecl) -> bool {
    let schema_ref = provider.config_schema_ref.to_ascii_lowercase();
    schema_ref.contains("setup") || schema_ref.contains("webhook") || schema_ref.contains("public")
}

fn validate_provider_decls(manifest: &PackManifest) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let providers = provider_decls(manifest);

    if providers.is_empty() {
        diagnostics.push(diagnostic(
            "error",
            "MSG_NO_PROVIDER_DECL",
            "Messaging pack must declare at least one provider.",
            Some(format!("extensions.{PROVIDER_EXTENSION_ID}.providers")),
            Some("Add provider declarations under the provider extension.".to_owned()),
        ));
        return diagnostics;
    }

    for (idx, provider) in providers.iter().enumerate() {
        let base_path = format!("extensions.{PROVIDER_EXTENSION_ID}.providers[{idx}]");

        if provider.provider_type.trim().is_empty() {
            diagnostics.push(diagnostic(
                "error",
                "MSG_PROVIDER_SCHEMA_EMPTY",
                "Provider declaration must include a non-empty provider type.",
                Some(format!("{base_path}.provider_type")),
                Some("Set the provider type identifier for this provider.".to_owned()),
            ));
        }

        if provider.ops.is_empty() {
            diagnostics.push(diagnostic(
                "error",
                "MSG_PROVIDER_NO_OPS",
                "Provider declaration must include at least one operation.",
                Some(format!("{base_path}.ops")),
                Some("Declare the operations exposed by this provider.".to_owned()),
            ));
        }

        if provider.config_schema_ref.trim().is_empty() {
            diagnostics.push(diagnostic(
                "error",
                "MSG_PROVIDER_CONFIG_PATH_EMPTY",
                "Provider config schema reference must not be empty.",
                Some(format!("{base_path}.config_schema_ref")),
                Some("Point to the provider configuration schema.".to_owned()),
            ));
        }
    }

    diagnostics
}

fn validate_setup_flow(manifest: &PackManifest) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();

    if !has_setup_entry(manifest) {
        diagnostics.push(diagnostic(
            "error",
            "MSG_SETUP_ENTRY_MISSING",
            "Setup flow declared but no setup entrypoint found.",
            Some("flows".to_owned()),
            Some("Ensure at least one flow entrypoint is named 'setup'.".to_owned()),
        ));
        return diagnostics;
    }

    let providers = provider_decls(manifest);
    let has_public_url = providers
        .iter()
        .any(|provider| provider_mentions_public_url(provider));
    if !has_public_url {
        diagnostics.push(diagnostic(
            "warn",
            "MSG_SETUP_PUBLIC_URL_NOT_ASSERTED",
            "Setup flow should assert a public webhook URL in provider config schema.",
            Some("extensions".to_owned()),
            Some(
                "Expose PUBLIC_BASE_URL (or equivalent) in the provider config schema.".to_owned(),
            ),
        ));
    }

    diagnostics
}

fn validate_subscriptions_flow(manifest: &PackManifest) -> Vec<Diagnostic> {
    if has_subscriptions_flow(manifest) {
        return Vec::new();
    }

    vec![diagnostic(
        "warn",
        "MSG_SUBSCRIPTIONS_DECLARED_BUT_NO_FLOW",
        "Subscriptions declared but no subscriptions flow found.",
        Some("flows".to_owned()),
        Some("Add a subscriptions flow (e.g. sync-subscriptions).".to_owned()),
    )]
}

fn validate_secret_requirements(manifest: &PackManifest) -> Vec<Diagnostic> {
    let providers = provider_decls(manifest);
    if providers.is_empty() || providers.iter().all(|provider| provider.ops.is_empty()) {
        return Vec::new();
    }

    if !manifest.secret_requirements.is_empty() {
        return Vec::new();
    }

    vec![diagnostic(
        "warn",
        "MSG_SECRETS_REQUIREMENTS_NOT_DISCOVERABLE",
        "Provider operations declared but secret requirements are not discoverable.",
        Some("secret_requirements".to_owned()),
        Some(
            "Include secret requirements in the manifest or reference secret requirements assets."
                .to_owned(),
        ),
    )]
}

fn diagnostic(
    severity: &str,
    code: &str,
    message: &str,
    path: Option<String>,
    hint: Option<String>,
) -> Diagnostic {
    Diagnostic {
        severity: severity.to_owned(),
        code: code.to_owned(),
        message: message.to_owned(),
        path,
        hint,
    }
}
