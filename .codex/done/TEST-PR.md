# PR: greentic-messaging — Extend greentic-messaging-test to run gtpack messaging provider packs (generic)

Goal
- Extend the existing `greentic-messaging-test` CLI (currently a MessageCard adapter/fixture validator) to ALSO run gtpack messaging provider packs from greentic-messaging-providers.
- Must remain provider-agnostic: NO provider-specific crates/types/logic added to greentic-messaging.
- The only “provider specifics” come from:
  - the gtpack itself (flows/components/config)
  - secrets resolved via gsm-core URIs (secrets://<env>/<tenant>/<team>/messaging/<provider>.credentials.json)
- Keep current commands intact (list/fixtures/adapters/run/all/gen-golden).
- Add a NEW command group/subcommand for packs.

Non-goals
- Do NOT add legacy provider crates to greentic-messaging.
- Do NOT add “GitHub secrets provider” or any new secrets backend. Use existing gsm-core resolver and existing greentic-secrets backends (file-seeded CI is fine).

Design
1) Add new top-level command: `packs`
   Subcommands:
   - `packs list`:
       list discovered packs and metadata (id, file path, maybe pack manifest name/version)
   - `packs run <pack_path>`:
       run a single gtpack pack smoke flow(s)
   - `packs all`:
       run all discovered packs (default: dry-run)
   Options for packs subcommands:
   - `--packs <DIR>` default `dist/packs` (or `./dist/packs`), allow multiple --packs
   - `--glob <PATTERN>` default `messaging-*.gtpack`
   - `--flow <FLOW_ID>` optional; if omitted, run `smoke` if present else run pack default entry flow
   - `--env <ENV>` default `dev`
   - `--tenant <TENANT>` default `ci`
   - `--team <TEAM>` default `ci`
   - `--dry-run` flag:
       when true, do not invoke outbound provider calls; validate flow wiring + scenario plan only
       (If you can’t fully “dry-run” provider calls generically, implement a safe dry-run mode by stubbing
        tool invocations at the executor boundary and recording intended calls without performing network IO.)
   - `--fail-fast` optional

2) Pack discovery
- Implement pack discovery by scanning one or more `--packs` directories for files matching `--glob`.
- For each pack path, attempt to decode pack manifest using existing greentic-types helpers (PackManifest decode).
- Keep discovery resilient: non-gtpack or invalid packs are listed with an error status but do not crash listing.

3) Pack execution (generic)
- Use the existing flow execution engine used by Greentic for packs (whatever module is responsible for:
  loading a gtpack, resolving components, executing flows).
- Execution must:
  - load pack
  - select a flow id (explicit --flow, else smoke if exists, else pack-defined default)
  - build a minimal test payload (e.g. a fixture message card or a generic “hello” event) as required by the flow input schema.
  - run the flow and report success/failure per pack
- Secrets:
  - rely on gsm-core resolver exactly as currently described in greentic-messaging-test docs:
    secrets://<env>/<tenant>/<team>/messaging/<provider>.credentials.json
  - Ensure env/tenant/team from CLI are used to build secret URIs.
  - Do not add special-case secret loading.
- Observability:
  - print a per-pack header and per-step results
  - on failure, print the flow id and step that failed, plus a short error chain

4) CI compatibility and “no provider bleed”
- `greentic-messaging-test packs all --dry-run` must be runnable in PR CI without secrets.
- Live CI (nightly/manual) will seed secrets via greentic-secrets init in the caller repo; greentic-messaging-test just reads via resolver.

5) Testing
- Unit tests for:
  - pack discovery/glob
  - selecting smoke flow fallback behavior
  - argument parsing
- Integration tests (offline) using a tiny sample gtpack in-repo under `tests/fixtures/packs/`:
  - include a minimal pack that has a smoke flow that does not require network calls
  - ensure `packs run` and `packs all --dry-run` work deterministically

6) Docs
- Update greentic-messaging-test help/README:
  - clarify it supports both “cards/adapters” and “packs”
  - show examples:
      greentic-messaging-test packs list --packs dist/packs
      greentic-messaging-test packs run dist/packs/messaging-telegram.gtpack --env dev --tenant ci --team ci --dry-run
      greentic-messaging-test packs all --packs dist/packs --glob 'messaging-*.gtpack' --dry-run
- Remove or fix stale docs that show calling the binary with a gtpack path as a positional command.

Implementation constraints
- Keep the CLI stable: existing subcommands must behave exactly as before.
- Keep dependencies minimal:
  - prefer reusing existing crates in greentic-* workspace (types, pack loader, runner, gsm-core)
  - avoid introducing new runtime deps unless required

Acceptance criteria
- `greentic-messaging-test --help` shows new `packs` command.
- `greentic-messaging-test packs list` lists gtpack files correctly.
- `greentic-messaging-test packs run <pack.gtpack> --dry-run` works without secrets for a pack that doesn’t need network calls.
- `greentic-messaging-test packs all --dry-run` can be used by greentic-messaging-providers CI to validate every pack.
- No provider crates are added back into greentic-messaging; pack execution remains generic.
