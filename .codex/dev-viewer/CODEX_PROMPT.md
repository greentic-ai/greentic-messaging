# Codex prompt: implement dev-viewer PRs (provider packs → platforms → operator send)

You are working in `greentic-messaging` repo, crate `dev-viewer`.

## Rules
- Implement the current PR document in `.codex/` exactly.
- Do not ask for permission for routine edits; just make them.
- Keep CI green: `cargo fmt`, `cargo clippy`, `cargo test`.
- Prefer small modules: `pack_loader`, `providers`, `platforms`, `convert`, `operator_send`.
- Add at least one test per PR that proves the key behavior.
- Produce a short PR summary + commands to validate.

## Current PR order
1) `.codex/PR-01-provider-loading.md`
2) `.codex/PR-02-dynamic-platforms.md`
3) `.codex/PR-03-operator-send.md`

Start with PR-01, complete it fully, then stop.
