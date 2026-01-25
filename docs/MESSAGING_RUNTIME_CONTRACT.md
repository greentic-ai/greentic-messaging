# Messaging runtime contract

`greentic-messaging serve` is the runtime entry point that the operator shells out to when it wants to run the messaging stack locally, in CI, or in a managed deployment. Operator automation and release tooling depend on this CLI, so the flags, env vars, and side effects below must stay stable until the operator owns the same surface.

## Command shape

| Argument | Description |
| --- | --- |
| `<kind>` | One of `ingress`, `egress`, `subscriptions`, or `pack`. `ingress`/`egress`/`subscriptions` require a `--platform` (e.g. `slack`, `teams`). `pack` boots the gateway/egress/runner trio in pack-driven mode. |
| `--tenant <tenant>` | Required. Drives the `TENANT` env var for every spawned service. |
| `--team <team>` | Optional. Defaults to `default`. Written to the `TEAM` env var. |
| `--pack <path>` | Repeatable. Accepts `.gtpack` or `.yaml` pack references. Used by `pack` mode (and some dev helpers) to seed `MESSAGING_ADAPTER_PACK_PATHS`. |
| `--packs-root <path>` | Defaults to `packs`. The root location that backs default packs plus the pack list passed via `--pack`. |
| `--no-default-packs` | Tells the CLI not to auto-load the packs shipped with `packs/`. When set, `MESSAGING_INSTALL_ALL_DEFAULT_ADAPTER_PACKS=false` and `MESSAGING_DEFAULT_ADAPTER_PACKS=` are exported for downstream binaries. |
| `--strict-adapters` | Errors fast if any adapter pack fails to load. |
| `--adapter <name>` | Overrides the egress adapter name via `MESSAGING_EGRESS_ADAPTER`. Only relevant for `egress` mode. |

## Environment expectations

The CLI sets and forwards the following environment variables whenever it launches a `cargo run -p <package>` child:

| Variable | Source | Notes |
| --- | --- | --- |
| `GREENTIC_ENV` | `current_env()` (`dev` when unset) | Controls the logical deployment scope. |
| `TENANT` | `--tenant` value | Required for every child process. |
| `TEAM` | `--team` value (default `default`) | Always populated, even when the flag is omitted. |
| `NATS_URL` | `NATS_URL` env (default `nats://127.0.0.1:4222`) | Runtime services connect to this broker. Operator must override it for production systems. |
| `MESSAGING_PACKS_ROOT` | `--packs-root` (default `packs`) | Where CLI and services look for `.gtpack` bundles. |
| `MESSAGING_ADAPTER_PACK_PATHS` | Joined list of `--pack` paths | Present only when `--pack` is used. |
| `MESSAGING_INSTALL_ALL_DEFAULT_ADAPTER_PACKS` | Set to `false` when `--no-default-packs` | Ensures downstream services do not load the bundled default list. |
| `MESSAGING_DEFAULT_ADAPTER_PACKS` | Set to empty string when `--no-default-packs` | Complements the flag above. |
| `MESSAGING_EGRESS_ADAPTER` | `--adapter` value | Overrides the egress adapter when provided. |

## Runtime behavior

- `serve <kind>` builds a `ServeEnvConfig` that captures the env scope, tenant/team, NATS URL, pack roots, and CLI switches.
- Non-`pack` kinds simply spawn the matching daemon via `cargo run -p gsm-gateway`, `gsm-egress`, or the platform-specific `gsm-subscriptions-teams`. The CLI prints `Starting ... (env=..., tenant=..., team=..., nats=..., packs_root=...)` before the child is launched.
- `serve pack` loads the adapter registry via `load_adapter_registry_for_cli`. If no adapters can be loaded the command exits with an error (operator relies on this early failure).
- `serve pack` inspects the adapters to determine whether to start gateway/egress machinery (`MessagingAdapterKind::Ingress` adds `gsm-gateway`, `MessagingAdapterKind::Egress` adds `gsm-egress`, and `teams` platforms also start `gsm-subscriptions-teams`). Unknown providers emit a warning but still log the desired packages.
- Child services inherit the `GREENTIC_ENV`, `TENANT`, `TEAM`, `NATS_URL`, `MESSAGING_PACKS_ROOT`, and any pack-related env vars described above. Operator automation depends on those env names to configure downstream glue (NATS subjects, pack loaders, etc.).
- The CLI only depends on `cargo run -p` to launch the runtime binaries, so the `gsm-*` crates must remain buildable and their package names must not change until operator stops shelling out here.
- Changing the `serve` subcommand signature, required flags, or the env vars above risks breaking the operator’s ability to bootstrap the messaging stack or to ship new validator packs through the same pipeline.

## Operator checklist

1. Invoke `greentic-messaging serve pack --tenant <tenant> --pack <messaging-xyz.gtpack> --packs-root <path>` (add `--team`/`--platform` as needed) to spin up the runtime services.
2. Keep `NATS_URL`, `TENANT`, `GREENTIC_ENV`, and `--pack` arguments in sync with the packs you are validating.
3. Respect `--no-default-packs` when you only want the packs you pass explicitly.
4. Avoid changing the CLI’s `serve` contract until equivalent logic exists inside greentic-operator; otherwise the operator may ship incompatible arguments to the runtime helpers.
