# PR-02: Dynamic platform previews loaded from packs (no hardcoded platforms)

## Goal
Remove hardcoded “platform previews” from dev-viewer and instead **discover platforms from loaded provider packs**.
Render each platform preview dynamically based on what the pack says it supports.

### User story
A future where we add new platforms without touching dev-viewer code:
- Pack adds a new platform conversion
- dev-viewer automatically shows a new preview card

## Dependencies
- PR-01 merged (providers load + registry exists)

## Scope
1. Define a platform registry discovered from packs
2. Replace fixed preview list with dynamic rendering
3. Conversion pipeline must resolve converter from pack metadata
4. Per-platform error reporting inside its card

## Non-goals
- No operator “Send test message” button yet (PR-03).
- No recursive pack discovery enhancements.

---

## Implementation plan

### 1) Platform model + registry
Create module: `dev_viewer::platforms`
- `PlatformId` (string)
- `PlatformDescriptor`
  - `id` (e.g. `slack`, `bf_webchat`, etc.)
  - `label`
  - `provider_id` (ties to selected provider or “all”)
  - `converter_ref` (whatever identifies the converter component/world in your pack)
  - `capabilities` (optional): e.g. supports_actions, supports_markdown, etc.
- `PlatformRegistry`
  - `platforms: Vec<PlatformDescriptor>`
  - `errors: Vec<PlatformDiscoveryError>`

Discovery rule:
- For each loaded pack/provider, read the provider-extension metadata and collect the supported platforms and converter references.

### 2) Refactor conversion interface to be generic
Create module: `dev_viewer::convert`
- `convert_fixture_to_ir(fixture: Fixture) -> NormalizedIr`
  - keep existing normalization
- `convert_ir_to_platform(ir: &NormalizedIr, platform: &PlatformDescriptor, ctx: &PackContext) -> ConvertResult`
  - resolve converter component from `platform.converter_ref`
  - execute converter (WASM/component/host call) using existing runner harness, or a lightweight embedded runtime
  - return output + warnings/errors

`ConvertResult` should include:
- `output_mime` (e.g. `application/json`, `text/plain`)
- `output_text` (pretty-printed where possible)
- `warnings: Vec<String>`
- `errors: Vec<String>` (empty on success)

### 3) UI changes
- Replace “Platform Previews” fixed cards with:
  - a dynamic list derived from `PlatformRegistry.platforms`
- Add a filter toggle:
  - “Only platforms supported by selected provider” (default on)
- Each platform preview card shows:
  - platform label
  - converter source (pack/version) in small text
  - output + warnings/errors

### 4) Failure behavior
- If a platform conversion fails:
  - card shows error
  - other platform cards still render (no global crash)

---

## Files (suggested)
- `crates/dev-viewer/src/platforms.rs` (new)
- `crates/dev-viewer/src/convert.rs` (new)
- `crates/dev-viewer/src/ui/platform_previews.rs` (replace fixed list)
- `crates/dev-viewer/src/pack_loader.rs`
  - extend to extract platform metadata into registry

---

## Acceptance checks
- [ ] No hardcoded platform list remains in dev-viewer.
- [ ] Platform preview cards are derived from packs.
- [ ] Adding a new platform to a pack makes dev-viewer show it without code changes.
- [ ] Per-platform errors are shown in the respective card.
- [ ] Output shown is clearly “what comes out of the fixture for that platform”.

## Suggested tests
- Unit test: platform discovery from a known provider extension blob
- Integration test: load a test pack defining 2 platforms; UI state includes 2 cards (headless/state test).

