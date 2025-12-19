# Greentic Messaging CLI

The `greentic-messaging` binary is a thin convenience layer on top of the existing
Makefile and Cargo flows. It inspects the current environment, shells out to the
existing ingress/egress/runner targets, proxies the fixture/test tooling, and now
exposes a first wave of admin helpers.

Secrets are now managed via the `greentic-secrets` CLI (ctx + scaffold/wizard/apply).
Use `messaging-tenants` or `greentic-secrets` directly to seed credentials; legacy
env/`./secrets` helpers are being phased out.

Testing with seeds:

- Point tests at a greentic-secrets seed file via `MESSAGING_SEED_FILE=/path/to/seed.yaml`.
- Prefer `messaging/<platform>.credentials.json` entries in the seed; legacy env/`SECRETS_ROOT`
  fallbacks can be disabled with `MESSAGING_DISABLE_ENV=1` and/or `MESSAGING_DISABLE_SECRETS_ROOT=1`.
- If you build packs locally, ensure component artifacts exist under `target/components/*.wasm` before running `tools/generate_packs.sh`; see `docs/pack_generation.md`.

## Installation

`cargo run -p greentic-messaging-cli -- <command>` from the repo root is the
recommended entrypoint while the CLI evolves. Once the crate stabilises we can
decide if a `cargo install` workflow makes sense.

## Commands

### `greentic-messaging info`

Inspect the current workspace:

- Detects `GREENTIC_ENV` (defaults to `dev`).
- Prints the current `greentic-secrets` context (via `greentic-secrets ctx show`) and hints at `greentic-secrets init --pack ...` for seeding.
- Shows the ingress/egress/subscription binaries that are available via the Makefile targets.

Example:

```bash
cargo run -p greentic-messaging-cli -- info
```

### `greentic-messaging dev up`

Shells out to `make stack-up` in the current workspace to spin up the optional
docker/NATS stack. Failures are logged but do not abort the CLI so you can keep
issuing commands afterwards.

### `greentic-messaging serve ingress|egress|subscriptions`

Wraps the existing `make run-<kind>-<platform>` targets while ensuring
`GREENTIC_ENV`, `TENANT`, `TEAM`, and `NATS_URL` are set. The command logs the
effective environment before delegating to `make`.

```bash
# example: run Slack ingress for tenant acme/team default
cargo run -p greentic-messaging-cli -- serve ingress slack --tenant acme

# run Teams egress for tenant foo/team bar
cargo run -p greentic-messaging-cli -- serve egress teams --tenant foo --team bar
```

### `greentic-messaging flows run`

Thin wrapper around the runner invocation (currently `make run-runner`). It sets
`FLOW`, `PLATFORM`, `TENANT`, `TEAM`, and `GREENTIC_ENV` automatically.

```bash
cargo run -p greentic-messaging-cli -- flows run \
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
```

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
(`cargo run --manifest-path scripts/Cargo.toml --bin teams_setup -- …`). Example:

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

Wraps `cargo run --manifest-path scripts/Cargo.toml --bin telegram_setup -- …`.
Use it to resolve a Telegram chat handle into its numeric chat id and persist it.

### `greentic-messaging admin whatsapp setup`

Wraps `cargo run --manifest-path scripts/Cargo.toml --bin whatsapp_setup -- …`
for verifying phone metadata and recording the token/recipient combo locally.

### Dry-run mode

Set `GREENTIC_MESSAGING_CLI_DRY_RUN=1` to print the underlying Make/Cargo
commands without executing them. This is useful for verifying the wiring or
running the CLI inside integration tests without contacting external services.
