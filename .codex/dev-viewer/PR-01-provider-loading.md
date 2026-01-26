# PR-01: Provider pack loading (strict) + Provider dropdown

## Goal
Make dev-viewer reliably load provider `.gtpack` files at startup and populate the **Provider** dropdown.
If pack loading is requested and **no providers** can be loaded, dev-viewer must exit with a clear error.

### User story
Running:
`cargo run -p dev-viewer -- --packs-dir ../greentic-messaging-providers/dist/packs/`
should either:
- show providers in the dropdown, or
- fail at startup with a clear pack loading report (no “silent empty” state).

## Scope (what changes)
1. CLI flags validation and early errors
2. Pack discovery (`--packs-dir`, `--provider-pack`)
3. Provider extraction from packs into a registry
4. UI dropdown population + basic “load report” display
5. Minimal tests for discovery + parsing wiring

## Non-goals
- No changes to how platform previews are rendered yet (PR-02 handles that).
- No operator send integration yet (PR-03 handles that).
- No new pack schema design; use whatever provider-extension data exists today.

---

## Implementation plan

### 1) CLI + startup behavior
- Ensure args are parsed like:
  - `--packs-dir <PATH>` (repeatable ok, optional)
  - `--provider-pack <PATH>` (repeatable ok, optional)
- Add startup decision:
  - If any pack flags are provided AND provider registry ends empty ⇒ **exit(1)**.
  - If no pack flags provided ⇒ allow running, show “No packs configured” in UI.

### 2) Pack discovery
Create module: `dev_viewer::pack_loader`
- `discover_pack_paths(packs_dir: &Path) -> Vec<PathBuf>`
  - collect `*.gtpack` (non-recursive first; consider optional `--recursive` later)
  - stable sort by filename
- `load_pack(path: &Path) -> Result<LoadedPack, PackLoadError>`
  - open and parse manifest
  - extract provider-extension section (messaging)
  - return `LoadedPack { path, manifest_meta, providers, ... }`

### 3) Provider registry
Create module: `dev_viewer::providers`
- `ProviderDescriptor`
  - `id` (route, e.g. `messaging.slack`)
  - `label` (human readable)
  - `pack_name`, `pack_version` (or digest)
  - `source_path` (for diagnostics)
- `ProviderRegistry`
  - `providers: Vec<ProviderDescriptor>`
  - `errors: Vec<PackLoadReportItem>`

Rules:
- Deduplicate providers by `id` (if collisions, keep first and log warning with both pack paths).

### 4) UI wiring
- Provider dropdown is driven by `ProviderRegistry.providers`.
- Display a status line:
  - “Loaded X providers from Y packs”
- If errors exist:
  - show expandable list or a scrollable “Pack load errors” panel.

### 5) Logging and error messages
- On startup, print a pack loading summary to stderr/log:
  - attempted packs (paths)
  - loaded providers (ids)
  - failures with concise causes
- Make errors actionable:
  - “Pack `<path>`: failed to parse manifest: <reason>”
  - “Pack `<path>`: provider-extension missing messaging providers”

---

## File-level checklist (suggested)
- `crates/dev-viewer/src/main.rs`
  - parse flags, build PackContext, enforce strict mode rules
- `crates/dev-viewer/src/pack_loader.rs` (new)
- `crates/dev-viewer/src/providers.rs` (new)
- `crates/dev-viewer/src/ui/*.rs`
  - wire dropdown to registry and show status/errors

(Adjust paths to match your repo layout.)

---

## Acceptance checks
- [ ] With a valid `--packs-dir`, providers appear in dropdown.
- [ ] With invalid pack(s), dev-viewer prints load report and exits(1) if zero providers load.
- [ ] With no pack flags, app starts but clearly indicates packs are not configured.
- [ ] Provider dropdown ordering is stable/deterministic.
- [ ] At least one automated test covers discovery + “strict startup error when empty providers”.

## Suggested tests
- Unit test for `discover_pack_paths` (sort + filter).
- Integration test using a small fixture `.gtpack` (or existing fixture pack in repo) to ensure provider extraction yields expected provider ids.

