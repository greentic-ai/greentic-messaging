# Audit overview

## Intent
- `greentic-messaging` started as the serverless-ready messaging runtime: ingress/egress/runner/subscriptions services, shared libs, and pack-aware tooling all live here so the CLI can bring up a full gateway/runner stack with provider packs (`README.md:2-69`).
- The repository still exposes local preview helpers (dev-viewer, translator fixtures, messaging-test) and telemetry/security kitchensink so teams can iterate on cards before handing them to the operator or providers (`README.md:116-170`).

## Current reality
- The repo now serves two purposes: (1) it builds and publishes the messaging-pack-validator component, pack bundle, and supporting `greentic-pack doctor` entrypoint that define the single compatibility gate, and (2) it keeps a second-class runtime/CLI that operator and providers can still call for local experimentation, e2e fixtures, and obsolete scripts (`docs/pack-validation.md:1-37`, `README.md:32-170`, `ci/local_check.sh:249-295`).
- Provider engineers still invoke `greentic-messaging-test` for e2e smoke, dry-run CI, and the `validate_gtpack_flows.sh` helper referenced in their docs, but every reference ends with “packs conformance” or “packs all --dry-run,” meaning the gate is the validator pack plus `greentic-pack doctor` rather than the legacy harness (`../greentic-messaging-providers/docs/ci_e2e.md:1-41`, `../greentic-messaging-providers/docs/ci_live_e2e.md:1-41`, `../greentic-messaging-providers/tools/validate_gtpack_flows.sh:11-29`).

## Where orchestration is moving
- `greentic-operator` now claims the orchestration surface: its demo yaml and config defaults point at the `gsm-*` binaries and the `greentic-messaging` command, so operator is responsible for starting the gateway/egress/subscriptions stack instead of each binary knowing how to bootstrap itself (`../greentic-operator/README.md:80-135`, `../greentic-operator/src/config.rs:345-368`, `../greentic-operator/src/services/messaging.rs:17-83`).
- In practice, operator shells out to `greentic-messaging serve ...` and the `gsm-*` executables while keeping its own runner/worker state machine, which keeps the messaging repo in the legacy bucket until operator re-implements these services internally.

## Single conformance gate
- The only compatibility gate today is the `greentic-messaging-pack-validator` component, the validator pack that embeds `greentic:pack-validate/pack-validator@0.1.0`, and the publish pipeline that builds it, zips it, runs `greentic-pack doctor`, and pushes the `.gtpack` to GHCR (`validators/messaging/pack.yaml:1-30`, `scripts/build-validator-component.sh:22-52`, `scripts/build-validator-pack.sh:4-81`, `.github/workflows/push-validator.yml:13-72`).
- CI and local-check wrap these scripts so that `greentic-pack doctor` is always the conformance gate rather than the old `messaging-test` harness (`ci/local_check.sh:249-295`).

## Keep vs legacy at a glance
- **Keep:** the validator component, pack definition, build scripts, and the push-validator workflow remain the only parts tied to the single gate; operator or greentic-pack doctor can’t be replaced without them.
- **Legacy:** everything else (runtime apps, CLI, translator libs, messaging-test harness, dev tooling, conformance helpers) is now either duplicated by operator, superseded by `greentic-pack doctor`, or only used for local/dev workflows that will move into the operator stack.
