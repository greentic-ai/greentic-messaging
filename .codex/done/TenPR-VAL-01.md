# TenPR-MSG-VAL-01 — Messaging domain pack validators (provider packs)

REPO: greentic-ai/greentic-messaging

GOAL
Add messaging-domain pack validators (for provider packs like messaging-telegram, messaging-slack, etc.) that:
- detect messaging provider packs reliably
- validate provider declaration completeness (ops/config schema references)
- validate setup/subscriptions expectations when the pack declares those entry flows
- validate presence/structure of secrets requirements assets when provider ops imply auth

All diagnostics MUST use the generic model from greentic-types.

NON-GOALS
- Do not modify greentic-types or greentic-pack in this PR.
- Do not execute WASM.
- Do not make network calls.
- Do not change pack format.

DELIVERABLES

A) New crate
Create `crates/greentic-messaging-validate/` with:
- `Cargo.toml` depends on:
  - `greentic-types` (for `PackManifest`, `PackValidator`, `Diagnostic`, `Severity`, etc.)
  - `serde_json` (only if needed for schema sniffing)
- `lib.rs` exports:
  - `pub fn messaging_validators() -> Vec<Box<dyn PackValidator>>`

B) Validator set (implement these)

1) MessagingPackDetector (shared helper)
Implement helper function used by all validators:
- `fn is_messaging_pack(manifest: &PackManifest) -> bool`
Return true if:
- manifest.meta.pack_id or name starts with "messaging-" OR
- manifest has providers where provider.schema starts with "greentic:provider/" AND provider id starts with "messaging." OR
- any config schema path contains "schemas/messaging/"
Use best-effort, but deterministic.

2) MessagingProviderDeclValidator
Applies if `is_messaging_pack(manifest) == true` AND providers list non-empty.
Checks:
- pack has at least one provider entry; else Error `MSG_NO_PROVIDER_DECL`
- for each provider:
  - ops count must be > 0 (if field exists); else Error `MSG_PROVIDER_NO_OPS`
  - if provider declares config schema path (e.g., `schemas/messaging/.../config.schema.json`), ensure it is non-empty string; else Error `MSG_PROVIDER_CONFIG_PATH_EMPTY`
  - if provider schema is empty -> Error `MSG_PROVIDER_SCHEMA_EMPTY`
Note: do NOT check file existence here unless greentic-types PackValidator API provides pack file list context. This PR focuses on manifest-level rules.

3) MessagingSetupFlowContractValidator
Applies if `is_messaging_pack(manifest)` and manifest.meta.entry_flows contains "setup" OR flow ids include "setup_*".
Checks:
- there must be at least one flow with `entry == "setup"` OR meta.entry_flows contains "setup"; else Error `MSG_SETUP_ENTRY_MISSING`
- pack should expose a config input for public URL:
  - If provider config schema path exists, emit Warn `MSG_SETUP_PUBLIC_URL_NOT_ASSERTED` unless the schema path name indicates it includes setup/public url (best-effort):
    - accept if config schema path contains "setup" OR "webhook" OR "public"
  - If pack includes `assets/secret-requirements.json`, that is not sufficient for public URL; still warn.
This is Warn-only to avoid breaking existing packs while you standardize PUBLIC_BASE_URL contract later.

4) MessagingSubscriptionsContractValidator
Applies if `is_messaging_pack(manifest)` and (flow ids contain "subscription" OR provider annotations mention subscriptions if available).
Checks:
- require at least one of:
  - flow id contains "sync-subscriptions"
  - flow id contains "subscriptions"
  - meta.entry_flows contains "subscriptions"
If none: Warn `MSG_SUBSCRIPTIONS_DECLARED_BUT_NO_FLOW`
(Again Warn-only initially.)

5) MessagingSecretRequirementsPresenceValidator
Applies if `is_messaging_pack(manifest)` and providers ops > 0.
Checks:
- pack annotations/imports (if present) OR asset list (if present in manifest model) indicates secrets.
If pack has `assets/secret-requirements.json` referenced in any way (manifest metadata or known path list in pack model), OK.
If not provable from manifest model, emit Warn `MSG_SECRETS_REQUIREMENTS_NOT_DISCOVERABLE`
(Do not error because file presence is best validated by greentic-pack core referenced-file validator.)

C) Tests
Add tests under `crates/greentic-messaging-validate/tests/` using minimal manifest fixtures:
- Build minimal `PackManifest` structs in-code (do not require real gtpack unzip).
Cases:
1) Non-messaging pack -> validators do nothing (no diags)
2) Messaging pack with no providers -> Error MSG_NO_PROVIDER_DECL
3) Messaging pack with setup entry but no setup flow -> Error MSG_SETUP_ENTRY_MISSING
4) Messaging pack with setup -> Warn MSG_SETUP_PUBLIC_URL_NOT_ASSERTED (unless schema path contains webhook/public/setup)
5) Messaging pack with "subscriptions" in flow ids -> no WARN in #4 for subscriptions

D) Docs
Add `docs/validation.md` describing the messaging validator codes and what they mean.

ACCEPTANCE
- `cargo test` passes
- crate exposes `messaging_validators()`
- only uses greentic-types validation model (no local duplicate Diagnostic types)
