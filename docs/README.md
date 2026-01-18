# Greentic Messaging

This doc is the single source of truth for the current messaging stack.

## Architecture: gateway -> runner -> egress

- Gateway accepts inbound webhook traffic, normalizes it into envelopes, and publishes to NATS.
- Runner consumes ingress envelopes, executes the flow declared by the pack, and emits outbound envelopes.
- Egress consumes outbound envelopes, renders provider payloads, and delivers them to the target platform.

The hop-by-hop flow is always: ingress gateway -> runner -> egress, with NATS as the transport.

## Packs

Packs are the contract between services and adapters. A pack can be a `.gtpack` bundle or a
`pack.yaml` during development. Packs declare:

- Provider extension metadata (provider type, component ref, capabilities).
- Flow IDs/paths for ingress and egress execution.
- Component artifacts (Wasm) needed by the runner.

Defaults live under `packs/messaging`. Override or add more packs with:

- `--pack <path>` (CLI flag, repeatable).
- `MESSAGING_ADAPTER_PACK_PATHS` (comma-separated paths).
- `MESSAGING_PACKS_ROOT` (root for default packs).

Use `tools/generate_packs.sh` to build `.gtpack` bundles.

## Dev CLI

Use `greentic-messaging` for local workflows:

- `info` shows env, secrets context, and resolved packs.
- `dev up` starts gateway, runner, and egress with the local NATS stack.
- `dev logs` tails component logs with prefixes.
- `dev setup <provider>` runs greentic-secrets and greentic-oauth for a provider.
- `serve ingress|egress|subscriptions|pack` launches services with pack-aware env wiring.
- `test ...` proxies `greentic-messaging-test`.
- `admin guard-rails` and `admin slack oauth-helper` expose admin helpers.

Defaults for `dev up`:

- `--tunnel cloudflared` (auto-detects `PUBLIC_BASE_URL`).
- `--subscriptions` on (skips if unsupported).
- `--packs-root ./packs`.

## DLQ

Failures are recorded in the DLQ (JetStream). Use the DLQ CLI to inspect and replay:

```bash
cargo run -p gsm-cli-dlq -- list --tenant acme --stage ingress
cargo run -p gsm-cli-dlq -- show 100
cargo run -p gsm-cli-dlq -- replay --tenant acme --stage egress --to ingress
```
