# Audit dependencies

## Cargo metadata (workspace member list)
- Command: `CARGO_NET_OFFLINE=true cargo metadata --format-version 1 --no-deps`
- Output: saved as `docs/AUDIT_METADATA.json` (contains all 27 workspace members and their dependency lists).

## Validator dependency closure
- Command: `cargo tree -p greentic-messaging-pack-validator`
- Output: saved as `docs/AUDIT_VALIDATOR_TREE.txt`, showing that the validator only pulls in `greentic-types`, `wit-bindgen`, and their transitive deps before producing the wasm component.

## Cross-repo imports
- `rg -n "greentic-messaging" ../greentic-operator`
  - `../greentic-operator/src/services/messaging.rs:17-83` calls `start_messaging_with_command(..., "greentic-messaging")`, i.e., operator still launches this repo’s CLI.
  - `../greentic-operator/src/config.rs:345-368` and `../greentic-operator/README.md:80-135` default `gsm-gateway`, `gsm-egress`, and `gsm-msgraph-subscriptions` binaries to the ones defined here.
- `rg -n "greentic-messaging-test" ../greentic-messaging-providers`
  - `../greentic-messaging-providers/docs/ci_e2e.md:1-30` and `../greentic-messaging-providers/docs/ci_live_e2e.md:1-41` both run `greentic-messaging-test packs ... --dry-run`.
  - `../greentic-messaging-providers/tools/validate_gtpack_flows.sh:11-29` insists on the CLI before exercising packs.
- `rg -n \"gsm-gateway\" ../greentic-operator` / `rg -n \"gsm-egress\" ../greentic-operator`
  - Operator config hardcodes these binaries, so removing them breaks the current operator orchestrator (`../greentic-operator/README.md:105-135`, `../greentic-operator/src/config.rs:345-366`).

## CI references
- `ci/local_check.sh` runs `cargo run -p greentic-messaging-test -- all --dry-run` and `scripts/build-validator-pack.sh`, mirroring the gating sequence described in `ci/README.md:1-9` and enforcing the gate offline (`ci/local_check.sh:249-275`).
- `.github/workflows/ci.yml` downloads `greentic-tools`, runs `cargo run -p greentic-messaging-test ...`, and exercises the same pack conformance commands referenced in the provider docs (`.github/workflows/ci.yml:56-110`).
- `.github/workflows/messaging-test.yml` reruns `greentic-messaging-test` for snapshots whenever that crate changes (`.github/workflows/messaging-test.yml:1-23`).
- `.github/workflows/push-validator.yml` builds + publishes the validator pack via `scripts/build-validator-pack.sh` (`.github/workflows/push-validator.yml:13-72`).
