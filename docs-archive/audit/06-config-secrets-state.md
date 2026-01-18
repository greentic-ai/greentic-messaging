# 06-config-secrets-state

## Config loading (env-first)

- gsm-gateway config is fully env-driven (`GREENTIC_ENV`, `NATS_URL`, `MESSAGING_GATEWAY_ADDR/PORT`, subject prefixes, worker routing). (Evidence: `apps/messaging-gateway/src/config.rs:20-64`; `docs/audit/_evidence/rg/config.txt`)
- gsm-egress config is env-driven (`GREENTIC_ENV`, `NATS_URL`, `MESSAGING_EGRESS_SUBJECT`, `MESSAGING_EGRESS_OUT_PREFIX`, `MESSAGING_PACKS_ROOT`, `MESSAGING_RUNNER_HTTP_URL/API_KEY`). (Evidence: `apps/messaging-egress/src/config.rs:17-41`; `docs/audit/_evidence/rg/config.txt`)
- gsm-runner uses env `NATS_URL`, `TENANT`, `PLATFORM`, `CHAT_PREFIX`, `FLOW` to configure subscriptions and flow selection. (Evidence: `apps/runner/src/main.rs:24-33`; `docs/audit/_evidence/rg/config.txt`)
- greentic-messaging CLI builds env vars for spawned services (e.g., `TENANT`, `TEAM`, `NATS_URL`, `MESSAGING_PACKS_ROOT`). (Evidence: `crates/greentic-messaging-cli/src/main.rs:534-564`; `docs/audit/_evidence/rg/tenant_context.txt`)

## Secrets mechanisms

- `DefaultResolver` from `secrets-core` implements `SecretsResolver` for JSON get/put using secret URIs; used across services. (Evidence: `libs/core/src/prelude.rs:1-63`; `docs/audit/_evidence/rg/secrets.txt`)
- Slack OAuth helper stores workspace tokens via `DefaultResolver` using `slack_workspace_secret` and `slack_workspace_index`. (Evidence: `apps/slack_oauth/src/main.rs:48-74`; `docs/audit/_evidence/rg/secrets.txt`)
- WebChat provider reads secrets by `Scope` (env/tenant/team) from `SecretsBackend` using `SecretUri`. (Evidence: `libs/core/src/platforms/webchat/provider.rs:85-206`; `docs/audit/_evidence/rg/secrets.txt`)
- Test utilities load credentials from `MESSAGING_SEED_FILE` or env/SECRETS_ROOT fallbacks. (Evidence: `libs/testutil/src/lib.rs:47-229`; `docs/audit/_evidence/rg/secrets.txt`)

## Process-global secret usage (env)

- Slack send adapter reads `SLACK_BOT_TOKEN` from process environment to populate HTTP auth. (Evidence: `libs/gsm-provider-registry/src/providers/slack/mod.rs:41-53`; `docs/audit/_evidence/rg/secrets.txt`)
- Slack OAuth server reads `SLACK_CLIENT_ID`, `SLACK_CLIENT_SECRET`, `SLACK_REDIRECT_URI`. (Evidence: `apps/slack_oauth/src/main.rs:92-101`; `docs/audit/_evidence/rg/secrets.txt`)

## State usage (config-adjacent)

- Session store is currently in-memory (`greentic-session` inmemory backend) via `store_from_env`. (Evidence: `libs/session/src/lib.rs:6-24`; `docs/audit/_evidence/rg/state_store.txt`)
- DLQ uses JetStream stream and env-configured subject formats for persistence. (Evidence: `libs/dlq/src/lib.rs:32-141`; `docs/audit/_evidence/rg/state_store.txt`)
- Idempotency uses JetStream KV buckets (with in-memory fallback) and TTL config from env. (Evidence: `libs/idempotency/src/lib.rs:84-188`; `apps/ingress-common/src/idempotency.rs:9-21`; `docs/audit/_evidence/rg/state_store.txt`)

## Known Unknowns

- The exact storage layout for secrets and sessions inside external crates (`greentic-secrets-core`, `greentic-session`) is not visible here. (Evidence: `libs/core/src/prelude.rs:1-63`; `docs/audit/_evidence/cargo-metadata.json:1`)
