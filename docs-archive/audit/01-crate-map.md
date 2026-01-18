# 01-crate-map

## Workspace layout (members)

Workspace members are defined in `Cargo.toml` and confirmed by `cargo metadata`. (Evidence: `Cargo.toml:1-31`; `docs/audit/_evidence/cargo-metadata.json:1`)

- Apps (runtime services/CLIs): `apps/runner`, `apps/ingress-common`, `apps/egress-common`, `apps/messaging-egress`, `apps/messaging-gateway`, `apps/messaging-tenants`, `apps/cli-dlq`, `apps/slack_oauth`. (Evidence: `Cargo.toml:1-31`; `docs/audit/_evidence/cargo-metadata.json:1`)
- Libs (shared logic): `libs/core`, `libs/session`, `libs/translator`, `libs/security`, `libs/telemetry`, `libs/testutil`, `libs/idempotency`, `libs/backpressure`, `libs/dlq`, `libs/gsm-provider-registry`. (Evidence: `Cargo.toml:1-31`; `docs/audit/_evidence/cargo-metadata.json:1`)
- Crates (CLI/bus/test harness): `crates/greentic-messaging-cli`, `crates/messaging-test`, `crates/gsm-bus`. (Evidence: `Cargo.toml:1-31`; `docs/audit/_evidence/cargo-metadata.json:1`)
- Tools & conformance: `tools/nats-demo`, `tools/mock-weather-tool`, `tools/dev-viewer`, `conformance/webchat`. (Evidence: `Cargo.toml:1-31`; `docs/audit/_evidence/cargo-metadata.json:1`)

## Workspace binaries (purpose + boot path + inputs/outputs)

### greentic-messaging (CLI wrapper)
- Purpose: CLI wrapper for dev stack, serve, flows, tests, and admin helpers. (Evidence: `crates/greentic-messaging-cli/src/main.rs:60-180`; `docs/audit/_evidence/rg/tenant_context.txt`)
- Boot path: `main` -> `Cli::parse` -> `handle_*` dispatch. (Evidence: `crates/greentic-messaging-cli/src/main.rs:20-55`; `docs/audit/_evidence/rg/config.txt`)
- Inputs: CLI flags (serve/dev/flows/test/admin), env `GREENTIC_ENV`, `NATS_URL`, pack flags, etc. (Evidence: `crates/greentic-messaging-cli/src/main.rs:70-180`; `docs/audit/_evidence/rg/tenant_context.txt`)
- Outputs: stdout logging, spawns `cargo run`/`make`/`docker compose` to start services or tools. (Evidence: `crates/greentic-messaging-cli/src/main.rs:337-765`; `docs/audit/_evidence/rg/overlaps_wip.txt`)
- Dependencies: see `docs/audit/_evidence/cargo-tree/greentic-messaging.txt`. (Evidence: `docs/audit/_evidence/cargo-tree/greentic-messaging.txt:1`; `docs/audit/_evidence/cargo-metadata.json:1`)

### gsm-gateway (HTTP ingress gateway)
- Purpose: HTTP API that normalizes inbound requests and publishes ingress envelopes to NATS, with optional worker forwarding. (Evidence: `apps/messaging-gateway/src/http.rs:118-357`; `docs/audit/_evidence/rg/envelopes.txt`)
- Boot path: `main` -> `GatewayConfig::from_env` -> `run` (binds listener, builds router). (Evidence: `apps/messaging-gateway/src/main.rs:6-12`; `apps/messaging-gateway/src/main_logic.rs:13-70`; `docs/audit/_evidence/rg/config.txt`)
- Inputs: env `GREENTIC_ENV`, `NATS_URL`, `MESSAGING_GATEWAY_ADDR/PORT`, `MESSAGING_GATEWAY_DEFAULT_TEAM`, `MESSAGING_INGRESS_SUBJECT_PREFIX`, worker routing envs. (Evidence: `apps/messaging-gateway/src/config.rs:20-64`; `docs/audit/_evidence/rg/config.txt`)
- Outputs: HTTP responses, NATS publish to ingress subjects, optional publish to worker egress subject. (Evidence: `apps/messaging-gateway/src/http.rs:268-330`; `docs/audit/_evidence/rg/envelopes.txt`)
- Dependencies: see `docs/audit/_evidence/cargo-tree/gsm-gateway.txt`. (Evidence: `docs/audit/_evidence/cargo-tree/gsm-gateway.txt:1`; `docs/audit/_evidence/cargo-metadata.json:1`)

### gsm-egress (egress worker)
- Purpose: JetStream consumer that reads `OutMessage`, invokes runner adapter, and publishes egress envelopes. (Evidence: `apps/messaging-egress/src/main_logic.rs:21-206`; `docs/audit/_evidence/rg/envelopes.txt`)
- Boot path: `main` -> `gsm_egress::run` -> NATS/JetStream loop. (Evidence: `apps/messaging-egress/src/main.rs:4-7`; `apps/messaging-egress/src/main_logic.rs:21-110`; `docs/audit/_evidence/rg/config.txt`)
- Inputs: env `GREENTIC_ENV`, `NATS_URL`, `MESSAGING_EGRESS_SUBJECT`, `MESSAGING_EGRESS_ADAPTER`, `MESSAGING_PACKS_ROOT`, `MESSAGING_RUNNER_HTTP_URL/API_KEY`. (Evidence: `apps/messaging-egress/src/config.rs:17-41`; `docs/audit/_evidence/rg/config.txt`)
- Outputs: NATS JetStream ack, publish to egress subject prefix. (Evidence: `apps/messaging-egress/src/main_logic.rs:84-205`; `docs/audit/_evidence/rg/envelopes.txt`)
- Dependencies: see `docs/audit/_evidence/cargo-tree/gsm-egress.txt`. (Evidence: `docs/audit/_evidence/cargo-tree/gsm-egress.txt:1`; `docs/audit/_evidence/cargo-metadata.json:1`)

### gsm-runner (flow runner)
- Purpose: NATS subscriber that processes `InvocationEnvelope` with flows and emits `OutMessage`. (Evidence: `apps/runner/src/main.rs:21-205`; `docs/audit/_evidence/rg/envelopes.txt`)
- Boot path: `main` -> load flow -> subscribe to subjects -> `handle_env` per message. (Evidence: `apps/runner/src/main.rs:21-83`; `docs/audit/_evidence/rg/envelopes.txt`)
- Inputs: env `NATS_URL`, `TENANT`, `PLATFORM`, `CHAT_PREFIX`, `FLOW`. (Evidence: `apps/runner/src/main.rs:24-33`; `docs/audit/_evidence/rg/config.txt`)
- Outputs: NATS publish to `greentic.msg.out...`, DLQ publish on failure, session writes. (Evidence: `apps/runner/src/main.rs:140-250`; `docs/audit/_evidence/rg/state_store.txt`)
- Dependencies: see `docs/audit/_evidence/cargo-tree/gsm-runner.txt`. (Evidence: `docs/audit/_evidence/cargo-tree/gsm-runner.txt:1`; `docs/audit/_evidence/cargo-metadata.json:1`)

### gsm-slack-oauth (Slack OAuth helper)
- Purpose: HTTP server for Slack OAuth install/callback storing workspace tokens. (Evidence: `apps/slack_oauth/src/main.rs:90-201`; `docs/audit/_evidence/rg/secrets.txt`)
- Boot path: `main` -> build `AppState` -> axum routes for `/slack/install` and `/slack/callback`. (Evidence: `apps/slack_oauth/src/main.rs:90-125`; `docs/audit/_evidence/rg/providers.txt`)
- Inputs: env `SLACK_CLIENT_ID/SECRET/REDIRECT_URI`, optional `SLACK_SCOPES`, `SLACK_USER_SCOPES`, `SLACK_API_BASE`, `BIND`. (Evidence: `apps/slack_oauth/src/main.rs:92-123`; `docs/audit/_evidence/rg/config.txt`)
- Outputs: HTTP redirects and JSON responses; writes secrets via `DefaultResolver`. (Evidence: `apps/slack_oauth/src/main.rs:48-74`; `docs/audit/_evidence/rg/secrets.txt`)
- Dependencies: see `docs/audit/_evidence/cargo-tree/gsm-slack-oauth.txt`. (Evidence: `docs/audit/_evidence/cargo-tree/gsm-slack-oauth.txt:1`; `docs/audit/_evidence/cargo-metadata.json:1`)

### gsm-cli-dlq (DLQ CLI)
- Purpose: List/show/replay DLQ entries via NATS. (Evidence: `apps/cli-dlq/src/main.rs:17-142`; `docs/audit/_evidence/rg/state_store.txt`)
- Boot path: `main` -> parse CLI -> call `gsm_dlq` helpers. (Evidence: `apps/cli-dlq/src/main.rs:70-139`; `docs/audit/_evidence/rg/state_store.txt`)
- Inputs: CLI flags for tenant/stage/limit/sequence, env `NATS_URL`. (Evidence: `apps/cli-dlq/src/main.rs:19-75`; `docs/audit/_evidence/rg/config.txt`)
- Outputs: stdout table/JSON; NATS reads/writes for replay. (Evidence: `apps/cli-dlq/src/main.rs:83-137`; `docs/audit/_evidence/rg/state_store.txt`)
- Dependencies: see `docs/audit/_evidence/cargo-tree/gsm-cli-dlq.txt`. (Evidence: `docs/audit/_evidence/cargo-tree/gsm-cli-dlq.txt:1`; `docs/audit/_evidence/cargo-metadata.json:1`)

### messaging-tenants (greentic-secrets wrapper)
- Purpose: Wrapper that shells out to `greentic-secrets` for messaging packs. (Evidence: `apps/messaging-tenants/src/main.rs:7-190`; `docs/audit/_evidence/rg/secrets.txt`)
- Boot path: `main` -> parse CLI -> build args -> spawn secrets CLI. (Evidence: `apps/messaging-tenants/src/main.rs:88-209`; `docs/audit/_evidence/rg/secrets.txt`)
- Inputs: CLI flags and env `GREENTIC_SECRETS_CLI`. (Evidence: `apps/messaging-tenants/src/main.rs:13-86`; `docs/audit/_evidence/rg/secrets.txt`)
- Outputs: stdout/stderr from child process; exit status. (Evidence: `apps/messaging-tenants/src/main.rs:193-209`; `docs/audit/_evidence/rg/secrets.txt`)
- Dependencies: see `docs/audit/_evidence/cargo-tree/messaging-tenants.txt`. (Evidence: `docs/audit/_evidence/cargo-tree/messaging-tenants.txt:1`; `docs/audit/_evidence/cargo-metadata.json:1`)

### dev-viewer (MessageCard preview server)
- Purpose: HTTP UI for rendering MessageCards across platforms. (Evidence: `tools/dev-viewer/src/main.rs:27-249`; `docs/audit/_evidence/rg/providers.txt`)
- Boot path: `main` -> load fixtures -> build router -> serve. (Evidence: `tools/dev-viewer/src/main.rs:48-101`; `docs/audit/_evidence/rg/config.txt`)
- Inputs: CLI flags `--listen`, `--fixtures`, env `OAUTH_BASE_URL`. (Evidence: `tools/dev-viewer/src/main.rs:27-75`; `docs/audit/_evidence/rg/config.txt`)
- Outputs: HTTP responses, logs. (Evidence: `tools/dev-viewer/src/main.rs:85-200`; `docs/audit/_evidence/rg/providers.txt`)
- Dependencies: see `docs/audit/_evidence/cargo-tree/dev-viewer.txt`. (Evidence: `docs/audit/_evidence/cargo-tree/dev-viewer.txt:1`; `docs/audit/_evidence/cargo-metadata.json:1`)

### nats-demo (NATS demo tool)
- Purpose: Publish a demo inbound message and subscribe to outbound messages. (Evidence: `tools/nats-demo/src/main.rs:7-62`; `docs/audit/_evidence/rg/envelopes.txt`)
- Boot path: `main` -> connect NATS -> subscribe -> publish. (Evidence: `tools/nats-demo/src/main.rs:7-18`; `docs/audit/_evidence/rg/envelopes.txt`)
- Inputs: env `NATS_URL`, `TENANT`. (Evidence: `tools/nats-demo/src/main.rs:10-13`; `docs/audit/_evidence/rg/config.txt`)
- Outputs: stdout logging of outbound payloads and NATS publishes. (Evidence: `tools/nats-demo/src/main.rs:21-61`; `docs/audit/_evidence/rg/envelopes.txt`)
- Dependencies: see `docs/audit/_evidence/cargo-tree/nats-demo.txt`. (Evidence: `docs/audit/_evidence/cargo-tree/nats-demo.txt:1`; `docs/audit/_evidence/cargo-metadata.json:1`)

### mock-weather-tool (HTTP mock server)
- Purpose: Mock weather API used by flows/tools. (Evidence: `tools/mock-weather-tool/src/main.rs:14-54`; `docs/audit/_evidence/rg/config.txt`)
- Boot path: `main` -> build axum router -> serve. (Evidence: `tools/mock-weather-tool/src/main.rs:14-25`; `docs/audit/_evidence/rg/config.txt`)
- Inputs: env `BIND`. (Evidence: `tools/mock-weather-tool/src/main.rs:19-24`; `docs/audit/_evidence/rg/config.txt`)
- Outputs: HTTP JSON responses, logs. (Evidence: `tools/mock-weather-tool/src/main.rs:28-53`; `docs/audit/_evidence/rg/config.txt`)
- Dependencies: see `docs/audit/_evidence/cargo-tree/mock-weather-tool.txt`. (Evidence: `docs/audit/_evidence/cargo-tree/mock-weather-tool.txt:1`; `docs/audit/_evidence/cargo-metadata.json:1`)

### greentic-messaging-test (test harness)
- Purpose: CLI harness for fixtures, adapters, and pack-based testing. (Evidence: `crates/messaging-test/src/cli.rs:7-132`; `docs/audit/_evidence/rg/config.txt`)
- Boot path: `main` -> parse CLI -> `RunContext::new` -> `execute`. (Evidence: `crates/messaging-test/src/main.rs:13-16`; `docs/audit/_evidence/rg/config.txt`)
- Inputs: CLI flags for fixtures/packs/runner URL/env/tenant/team, etc. (Evidence: `crates/messaging-test/src/cli.rs:7-132`; `docs/audit/_evidence/rg/tenant_context.txt`)
- Outputs: stdout/stderr from tests and adapter runs. (Evidence: `crates/messaging-test/src/main.rs:13-16`; `docs/audit/_evidence/rg/config.txt`)
- Dependencies: see `docs/audit/_evidence/cargo-tree/greentic-messaging-test.txt`. (Evidence: `docs/audit/_evidence/cargo-tree/greentic-messaging-test.txt:1`; `docs/audit/_evidence/cargo-metadata.json:1`)

## Examples / test harness bins in workspace crates

- `gsm-core` examples: `run_standalone` and `wasmtime_host` under `libs/core/examples/`. (Evidence: `libs/core/examples/run_standalone.rs:1`; `docs/audit/_evidence/cargo-metadata.json:1`)

## Legacy (non-workspace) crates in repo

- `legacy/` contains its own Cargo manifests and adapter apps (ingress/egress/subscriptions/mocks). These are not workspace members but are referenced by CLI wrappers and docs. (Evidence: `legacy/Cargo.toml:1`; `crates/greentic-messaging-cli/src/main.rs:879-889`; `docs/audit/_evidence/rg/overlaps_wip.txt`)

## Known Unknowns

- External crate internals (e.g., `greentic-types`, `greentic-session`) are not in this repo; their internal types/fields are inferred only from usage sites. (Evidence: `libs/core/src/prelude.rs:1-9`; `docs/audit/_evidence/cargo-metadata.json:1`)
