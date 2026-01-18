# 07-storage-and-caches

## Persistent stores

- NATS JetStream stream for egress: stream `messaging-egress-<env>` with WorkQueue retention. (Evidence: `apps/messaging-egress/src/main_logic.rs:46-71`; `docs/audit/_evidence/rg/state_store.txt`)
- DLQ JetStream stream `DLQ` with subject patterns derived from env or defaults. (Evidence: `libs/dlq/src/lib.rs:32-141`; `docs/audit/_evidence/rg/state_store.txt`)
- Idempotency store (JetStream KV): bucket name `JS_KV_NAMESPACE_IDEMPOTENCY` or default `idempotency`. (Evidence: `libs/idempotency/src/lib.rs:84-188`; `apps/ingress-common/src/idempotency.rs:9-21`; `docs/audit/_evidence/rg/state_store.txt`)
- Backpressure store (JetStream KV): bucket name `JS_KV_NAMESPACE_BACKPRESSURE` or default `rate-limits`. (Evidence: `libs/backpressure/src/lib.rs:371-378`; `docs/audit/_evidence/rg/state_store.txt`)

## In-memory stores / caches

- Session store is in-memory via `greentic-session` inmemory backend. (Evidence: `libs/session/src/lib.rs:6-24`; `docs/audit/_evidence/rg/state_store.txt`)
- Local idempotency fallback uses in-memory map with TTL. (Evidence: `libs/idempotency/src/lib.rs:58-118`; `docs/audit/_evidence/rg/state_store.txt`)
- Local backpressure limiter keeps per-tenant token buckets in memory. (Evidence: `libs/backpressure/src/lib.rs:115-179`; `docs/audit/_evidence/rg/state_store.txt`)
- Provider registry cache is an in-memory `DashMap` keyed by `ProviderKey`. (Evidence: `libs/core/src/registry.rs:23-44`; `docs/audit/_evidence/rg/providers.txt`)

## Key schema (examples)

- Idempotency key string is `tenant:platform:msg_id`. (Evidence: `libs/idempotency/src/lib.rs:25-37`; `docs/audit/_evidence/rg/state_store.txt`)
- Backpressure KV key uses `rate/{tenant}`. (Evidence: `libs/backpressure/src/lib.rs:287-323`; `docs/audit/_evidence/rg/state_store.txt`)
- DLQ subjects include tenant/stage (and optional platform). (Evidence: `libs/dlq/src/lib.rs:32-141`; `docs/audit/_evidence/rg/state_store.txt`)
- Ingress/egress subjects embed tenant (and team for ingress). (Evidence: `crates/gsm-bus/src/lib.rs:33-63`; `docs/audit/_evidence/rg/envelopes.txt`)

## Tenant context in keys

- Idempotency keys include tenant and platform. (Evidence: `libs/idempotency/src/lib.rs:25-37`; `docs/audit/_evidence/rg/tenant_context.txt`)
- Backpressure keys are tenant-scoped. (Evidence: `libs/backpressure/src/lib.rs:286-323`; `docs/audit/_evidence/rg/tenant_context.txt`)
- Provider registry keys include env/tenant/team. (Evidence: `libs/core/src/provider.rs:5-31`; `docs/audit/_evidence/rg/tenant_context.txt`)

## Known Unknowns

- The persistence details for external components (e.g., WASM adapter state) are not visible in this repo. (Evidence: `libs/core/src/adapter_registry.rs:143-188`; `docs/audit/_evidence/cargo-metadata.json:1`)
