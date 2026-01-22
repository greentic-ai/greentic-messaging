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

Pack generation now lives in the providers repo; this repo no longer builds provider components.

## Pack validation

Provider packs must declare the messaging validator extension. See `docs/pack-validation.md`.

## Dev CLI

Use `greentic-messaging` for local workflows:

- `info` shows env, secrets context, and resolved packs.
- `dev up` starts gateway, runner, and egress with the local NATS stack.
- `dev logs` tails component logs with prefixes.
- `dev setup <provider>` runs greentic-provision + greentic-secrets for a provider and writes the install record (use the provider id from the pack; `greentic-messaging info` shows it).
- `serve ingress|runner|egress|subscriptions` launches services with pack-aware env wiring.
- `test ...` proxies `greentic-messaging-test`.

Defaults for `dev up`:

- `--tunnel cloudflared` (auto-detects `PUBLIC_BASE_URL`).
- `--subscriptions` on (skips if unsupported).
- `--packs-root ./packs`.

## Messaging-test live pack runs

`greentic-messaging-test packs run --live` validates pack wiring and then submits a single
egress message through the runner HTTP API (`RUNNER_URL`, default
`http://localhost:8081/invoke`). Use `--runner-transport nats` to publish the egress
message on NATS instead (`NATS_URL`, default `nats://127.0.0.1:4222`).

If you see:

- `Connection refused` for `http://localhost:8081/invoke`: no runner HTTP service is running.
  Start whichever runner HTTP service you use in your environment, or set `RUNNER_URL` to it.
- `gsm-runner` prints `no flows loaded from pack metadata`: it loads flows from the pack
  metadata in the greentic root (default `./packs`). Ensure your `.gtpack` files are under
  `./packs/messaging` or update your greentic config root before starting `gsm-runner`.

For live sends you still need provider config + secrets:

- `--config` must include required config keys from the pack schema (e.g. `public_base_url`).
- required secrets must exist in the greentic-secrets dev store (or your configured secret
  backend), which `greentic-messaging-test` reads when `--live` is set.

To watch messages on NATS:

```bash
greentic-messaging-test packs log --env dev --tenant ci --team ci --nats-url nats://127.0.0.1:4222
```

## DLQ

Failures are recorded in the DLQ (JetStream). Use the DLQ CLI to inspect and replay:

```bash
cargo run -p gsm-cli-dlq -- list --tenant acme --stage ingress
cargo run -p gsm-cli-dlq -- show 100
cargo run -p gsm-cli-dlq -- replay --tenant acme --stage egress --to ingress
```

## Serving

Use the CLI to run services with pack-aware configuration:

```bash
greentic-messaging serve ingress slack --tenant acme --team default
greentic-messaging serve runner --tenant acme --team default
greentic-messaging serve egress --tenant acme --team default
greentic-messaging serve subscriptions --tenant acme --team default
```

Provide pack overrides as needed:

```bash
greentic-messaging serve ingress webchat \
  --tenant acme \
  --pack /abs/path/to/messaging-webchat.gtpack \
  --no-default-packs
```
