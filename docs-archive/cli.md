# Greentic Messaging CLI

The `greentic-messaging` binary is a convenience layer over the messaging services.
It inspects the current environment, runs the gateway/egress/subscription services
with your chosen adapter packs (prefer `.gtpack` bundles), proxies the
fixture/test tooling, and exposes a first wave of admin helpers. Pack loading is
gtpack-first; YAML is supported for development.

Secrets are now managed via the `greentic-secrets` CLI (ctx + scaffold/wizard/apply).
Use `messaging-tenants` or `greentic-secrets` directly to seed credentials; legacy
env/`./secrets` helpers are being phased out.

Testing with seeds:

- Point tests at a greentic-secrets seed file via `MESSAGING_SEED_FILE=/path/to/seed.yaml`.
- Prefer `messaging/<platform>.credentials.json` entries in the seed; legacy env/`SECRETS_ROOT`
  fallbacks can be disabled with `MESSAGING_DISABLE_ENV=1` and/or `MESSAGING_DISABLE_SECRETS_ROOT=1`.
- Pack generation now lives in the providers repo; this repo no longer builds provider components.

## Installation

Use the packaged binary (`greentic-messaging …`) for day-to-day work. While
developing locally you can still run `cargo run -p greentic-messaging -- <command>`
from the repo root.

## Commands

### `greentic-messaging info`

Inspect the current workspace:

- Detects `GREENTIC_ENV` (defaults to `dev`).
- Prints the current `greentic-secrets` context (via `greentic-secrets ctx show`) and hints at `greentic-secrets init --pack ...` for seeding.
- Lists adapters loaded from your packs. Flags mirror `serve`:
  - `--pack <path>`: repeatable; supports `.yaml` and `.gtpack`.
  - `--packs-root <dir>`: sets `MESSAGING_PACKS_ROOT` (default `./packs`).
  - `--no-default-packs`: disables loading shipped defaults.

Example:

```bash
greentic-messaging info --pack fixtures/packs/messaging-provider-bundle.gtpack --no-default-packs
```

### `greentic-messaging dev up|down`

Starts/stops the optional docker/NATS stack. If a `Makefile` is present, the CLI
shells out to `make stack-up`/`stack-down`; otherwise it uses an embedded
docker compose definition so the standalone binary works outside the repo.
Failures are logged but do not abort the CLI so you can keep issuing commands
afterwards.

### `greentic-messaging serve ingress|egress|subscriptions`

Launches the pack-aware services (`gsm-gateway`, `gsm-egress`, `gsm-subscriptions-*`)
with the right environment pre-populated. Flags:

- `--pack <path>`: repeatable; supports `.yaml` and `.gtpack`. Passed via `MESSAGING_ADAPTER_PACK_PATHS`.
- `--packs-root <dir>`: sets `MESSAGING_PACKS_ROOT` (default `./packs`).
- `--no-default-packs`: disables loading shipped defaults.
- `--adapter <name>`: egress-only override for `MESSAGING_EGRESS_ADAPTER`.

```bash
# run ingress with a local gtpack bundle
greentic-messaging serve ingress webchat \
  --tenant acme \
  --pack fixtures/packs/messaging-provider-bundle.gtpack \
  --no-default-packs

# run egress with a specific adapter name
greentic-messaging serve egress webchat --tenant acme --adapter webchat-main
```

Runner note: egress currently republishes envelopes to the bus; runner HTTP
invocation is available when `MESSAGING_RUNNER_HTTP_URL` is set (auth via
`MESSAGING_RUNNER_HTTP_API_KEY`). If unset, a logging client is used. Bus
publish still occurs for legacy consumers. You can run `make run-runner` (or
your deployed greentic-runner) against the flow referenced by your pack to
service those HTTP invocations.

### `greentic-messaging serve pack`

Auto-start services based on adapters discovered in your packs:
- Starts `gsm-gateway` if any adapters support ingress.
- Starts `gsm-egress` if any adapters support egress.
- Starts `gsm-subscriptions-teams` when Teams adapters are present.

```bash
greentic-messaging serve pack \
  --tenant acme \
  --pack dist/packs/messaging-telegram.gtpack \
  --no-default-packs
```

### `greentic-messaging flows run`

Thin wrapper around the runner invocation (currently `make run-runner`). It sets
`FLOW`, `PLATFORM`, `TENANT`, `TEAM`, and `GREENTIC_ENV` automatically.

```bash
greentic-messaging flows run \
  --flow examples/flows/weather_telegram.yaml \
  --platform telegram \
  --tenant acme
```

### `greentic-messaging test …`

Thin wrapper around the existing `greentic-messaging-test` crate. Each subcommand
maps directly to the equivalent `cargo run -p greentic-messaging-test -- …` call:

```bash
greentic-messaging test fixtures
greentic-messaging test adapters
greentic-messaging test run oauth_slack --dry-run
greentic-messaging test all --dry-run   # hard requirement enforced by the CLI
greentic-messaging test gen-golden
# Pack-backed fixture execution (invoke adapters via runner)
greentic-messaging test run card.basic \
  --pack /abs/path/to/messaging-telegram.gtpack \
  --runner-url http://localhost:8081/invoke \
  --chat-id -100123456 \
  --env dev --tenant acme --team default
greentic-messaging test all \
  --pack /abs/path/to/messaging-telegram.gtpack \
  --runner-url http://localhost:8081/invoke \
  --chat-id -100123456 \
  --env dev --tenant acme --team default
# gtpack-aware smoke checks
greentic-messaging test packs list --packs dist/packs
greentic-messaging test packs run dist/packs/messaging-telegram.gtpack --dry-run --env dev --tenant ci --team ci
greentic-messaging test packs all --packs dist/packs --glob 'messaging-*.gtpack' --dry-run
```
Component resolution for packs is enabled by default and materializes public OCI components through `greentic-distributor-client` into `~/.cache/greentic/materialized/<hash>`. Use `--no-resolve-components` to skip, `--allow-tags` to permit tag-only refs, or `--offline` to require cached artifacts and avoid network pulls.

When `--dry-run` is omitted from `test run …` the command will send real traffic,
just like invoking the crate manually.

### `greentic-messaging admin guard-rails …`

Human-readable helpers for the ingress guard rails defined in
`apps/ingress-common/src/security.rs`:

```bash
greentic-messaging admin guard-rails show
```

Prints whether bearer auth, HMAC validation, and signed action links are enabled,
including which header/algorithm would be used if active.

```bash
greentic-messaging admin guard-rails sample-env
```

Emits a commented-out `.env` snippet covering `INGRESS_BEARER`,
`INGRESS_HMAC_SECRET`, `INGRESS_HMAC_HEADER`, `JWT_SECRET`, `JWT_ALG`, and
`ACTION_BASE_URL`.

### `greentic-messaging admin slack oauth-helper`

Wraps the Slack OAuth helper (`cargo run -p gsm-slack-oauth -- …`). Any extra
arguments are forwarded verbatim after `--`, so existing flows keep working:

```bash
greentic-messaging admin slack oauth-helper -- --listen 0.0.0.0:8080
```

### `greentic-messaging admin teams setup`

Wraps the Teams chat verification tool
(`cargo run --manifest-path legacy/scripts/Cargo.toml --bin teams_setup -- …`). Example:

```bash
greentic-messaging admin teams setup -- \
  --tenant 11111111-2222-3333-4444-555555555555 \
  --client-id 9999-aaaa-bbbb-cccc \
  --client-secret super-secret \
  --chat-id 19:deadbeef@thread.v2
```

The command prints the validated chat metadata, mirrors the `.env` persistence
behaviour, and exits with the same status code as the helper binary.

### `greentic-messaging admin telegram setup`

Wraps `cargo run --manifest-path legacy/scripts/Cargo.toml --bin telegram_setup -- …`.
Use it to resolve a Telegram chat handle into its numeric chat id and persist it.

### `greentic-messaging admin whatsapp setup`

Wraps `cargo run --manifest-path legacy/scripts/Cargo.toml --bin whatsapp_setup -- …`
for verifying phone metadata and recording the token/recipient combo locally.

### Dry-run mode

Set `GREENTIC_MESSAGING_CLI_DRY_RUN=1` to print the underlying Make/Cargo
commands without executing them. This is useful for verifying the wiring or
running the CLI inside integration tests without contacting external services.
