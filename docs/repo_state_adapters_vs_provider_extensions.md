# Adapter Registry vs Provider Extensions (repo snapshot)

## Executive summary
- Adapter discovery now **prefers provider extensions (`greentic.provider-extension.v1`)**; legacy `messaging.adapters` is a fallback with a warning. (libs/core/src/adapter_registry.rs)
- CLI `info` lists adapters for packs with provider extensions; packs with neither extension nor legacy adapters show the empty hint.
- Registry loading is centralized in `AdapterRegistry::load_from_paths`, fed by CLI/Gateway/Egress helpers; paths are canonicalized and missing packs are skipped with a warning.
- Packs must carry either a provider extension or `messaging.adapters` to be usable. Provider extension takes precedence when both exist.

## Observed behavior (commands)
- Repo state: `git status --short` shows modified files; `HEAD`=467dd11e90871194265ad351321397c622e96080 on branch `master`. Tooling: `cargo 1.89.0`, `rg 15.1.0`.
- `greentic-messaging --help` outputs commands only (no adapter details).
- `greentic-messaging info --pack ../greentic-messaging-providers/dist/packs/messaging-slack.gtpack` → `adapters: (none loaded; add --pack or configure env)` (pack has no provider extension or legacy adapters).
- `greentic-messaging info --pack <synthetic provider-extension gtpack>` (test fixture) lists the provider-derived adapter, proving extension-first loading.

## Where adapters are loaded (code)
- **Adapter registry core**: `libs/core/src/adapter_registry.rs`
  - Loader entry: `AdapterRegistry::load_from_paths` (lines 63-116).
  - Pack dispatch: `adapters_from_pack_file` chooses YAML vs `.gtpack` (118-128).
  - GTpack read: `adapters_from_gtpack` calls `open_pack`, then `extract_adapters_from_manifest` (150-155, 170-180).
  - Extraction: `extract_from_sources` prefers provider extension inline payload (`greentic.provider-extension.v1`); if present, maps each provider to an adapter. Otherwise falls back to `messaging.adapters` with a WARN; warns when neither exists (182-330).
- **Gateway registry builder**: `apps/messaging-gateway/src/lib.rs:18-52` builds pack paths (defaults + env) then calls `load_adapters_from_pack_files`; logs when empty.
- **Egress registry builder**: `apps/messaging-egress/src/main_logic.rs:26-44` builds pack paths (defaults + env), loads registry, then proceeds (warns and uses empty registry on failure).
- **CLI registry builder**: `crates/greentic-messaging-cli/src/main.rs:765-801`
  - Collects default packs (unless `--no-default-packs`), env `MESSAGING_ADAPTER_PACK_PATHS`, and CLI `--pack`.
  - `canonicalize_pack_path` warns and skips paths that cannot be canonicalized (788-801).
  - Calls `AdapterRegistry::load_from_paths`; prints `(none loaded…)` when `is_empty()`.

## Whether extensions/providers are used
- Adapter extraction now reads provider extensions inline from the manifest (provider inline or extension entry) and maps them to adapters (libs/core/src/adapter_registry.rs:182-330).
- Legacy `messaging.adapters` is only used if no provider extension adapters exist and emits a deprecation warning.
- Packs lacking both sections yield no adapters with a warning.

## Call graph (CLI `info`)
`main` → `handle_info` (crates/greentic-messaging-cli/src/main.rs:232-279) → `load_adapter_registry_for_cli` (765-801) → `AdapterRegistry::load_from_paths` → `adapters_from_pack_file` → `adapters_from_gtpack`/`adapters_from_pack_yaml` → `extract_from_sources` (provider extension first, then legacy messaging). If registry is empty, `handle_info` prints the provider/legacy hint.

## Impact on packs
- A “usable” pack must include either a provider extension (`greentic.provider-extension.v1`) or a legacy `messaging.adapters` section. Provider extension takes precedence when both exist.
- Packs with only bare components (no extension, no legacy adapters) will not register adapters.

## Two forward paths
**Option A — Provider extension first (current path)**  
- Publish packs with provider extension populated (provider_type, runtime.component_ref, optional flows/capabilities). Keep legacy only as a fallback until removed.  
- Add release-time validation to fail packs missing both extension and legacy adapters.

**Option B — Hybrid with richer mapping**  
- Extend provider-extension mapping to carry flow/custom metadata (if schema evolves) and emit richer AdapterDescriptor fields (flows, capabilities).  
- Eventually drop legacy messaging.adapters support once downstream packs have migrated.

## Questions to ask / issues to raise
- Add CI/release checks: fail packs missing provider extension and legacy adapters; warn when legacy is used.
- Confirm provider-extension schema evolution for flows/capabilities so AdapterDescriptor can populate those fields without legacy adapters.
- Decide timeline to remove legacy `messaging.adapters` and stop logging warnings.
- Ensure default-pack behavior in CLI/Gateway/Egress is documented for third-party packs (consider enabling defaults by default or clearer hints).

Report path: `docs/repo_state_adapters_vs_provider_extensions.md`.
