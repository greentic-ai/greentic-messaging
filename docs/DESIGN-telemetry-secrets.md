# Telemetry, Secrets, and Tenant Context

This note captures the current state of telemetry/secrets/session wiring inside
`greentic-messaging` and highlights the migration hooks we will pursue when
aligning with the shared `greentic-*` crates.

## Tenant Context

- The canonical `TenantCtx` type comes from `greentic_types`.
- Every ingress creates one with `make_tenant_ctx` (see
  `libs/core/src/prelude.rs`) after loading `GREENTIC_ENV`, `TENANT`, and
  platform-specific hints (team/user/thread ids).
- `InvocationEnvelope` carries the same `TenantCtx` through runner → egress →
  subscriptions, so downstream services never re-derive tenant metadata.

## Secrets Resolution

- `libs/core/src/prelude.rs` defines the `SecretPath` helper plus the
  `SecretsResolver` trait, both backed by `greentic-secrets` types (`SecretUri`,
  `DefaultResolver`).
- Reads/writes use canonical greentic-secrets URIs such as
  `secrets://{env}/{tenant}/{team|_}/messaging/{platform}.credentials.json`.
- Writes follow the same pattern (e.g. admin helpers storing OAuth tokens).
- Platform-specific modules take `&impl SecretsResolver` so they stay agnostic to
  the concrete backend.

## Telemetry Wiring

- `libs/telemetry` (`gsm-telemetry`) now delegates subscriber installation and
  tenant-aware task locals to `greentic-telemetry`. The crate exposes the
  legacy helpers (`set_current_tenant_ctx`, `record_auth_card_render`, etc.) so
  the rest of the workspace does not need to depend on `greentic-telemetry`
  directly.
- `GREENTIC_DEV_TELEMETRY=1` flips `GT_TELEMETRY_FMT=1` under the hood so the
  shared subscriber emits structured stdout logs without requiring every binary
  to reimplement the dev-mode formatter.

## Session Store

- `libs/session` (`gsm-session`) wraps the shared `greentic-session` backends
  and exposes a `SharedSessionStore` handle with async-friendly helpers.
- `store_from_env()` constructs an in-memory store by default or switches to the
  Redis backend when `SESSION_REDIS_URL`/`SESSION_NAMESPACE` are set. All state
  is persisted as `greentic_session::SessionData`, ensuring runners/ingress use
  the same schema as the rest of the Greentic stack.

## Future Alignment with greentic-* crates

The main telemetry/session surfaces now reuse the shared crates. Remaining
follow-ups:

1. Audit the secrets stack to ensure `secrets_core::DefaultResolver` already
   points at greentic-secrets. If other services start needing rich filtering,
   consider inverting the dependency so the resolver lives upstream.
2. Continue migrating bespoke helpers (e.g., per-platform admin wiring) onto the
   shared crates as they add the required extension points.
