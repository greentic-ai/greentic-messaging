# TenPR-01 — Freeze and isolate legacy messaging paths

YOU ARE CODEX IN greentic-messaging.

GOAL  
Make legacy code impossible to reach by default, without deleting it yet.
This PR is mechanical and defensive: isolate legacy so new work is clean.

DO NOT redesign anything.
DO NOT remove files yet.
DO NOT change runtime behavior of legacy paths if explicitly enabled.

TASKS

1) Introduce a top-level Cargo feature:
   - feature name: `legacy`
   - legacy code must ONLY compile when this feature is enabled

2) Gate all legacy modules behind `#[cfg(feature = "legacy")]`:
   - legacy provider registries
   - legacy subject helpers (greentic.msg.in/out)
   - legacy CLIs and scripts
   - legacy config/secrets handling
   - legacy apps under `legacy/` or equivalent

3) Default build:
   - `legacy` feature OFF by default
   - `cargo build`, `cargo test` must pass without legacy

4) Any code that depends on legacy must:
   - either be gated
   - or refactored to call the new stack
   - or explicitly fail with a clear error if legacy is disabled

5) Add a short doc:
   - `docs/legacy.md`
   - Explain: legacy exists only for transition, is feature-gated, and will be deleted.

ACCEPTANCE
- `cargo build` works with legacy disabled
- `cargo build -F legacy` still works
- No new code depends on legacy paths
