# 03-tenancy-and-context

## Tenant/context types and definitions

- Core tenant identifiers are re-exported from `greentic-types`: `EnvId`, `TenantCtx`, `TenantId`, `TeamId`, `UserId`. (Evidence: `libs/core/src/prelude.rs:1-9`; `docs/audit/_evidence/rg/tenant_context.txt`)
- Provider registry keys bind tenant + env + team per platform via `ProviderKey`. (Evidence: `libs/core/src/provider.rs:1-31`; `docs/audit/_evidence/rg/tenant_context.txt`)
- Channel/egress envelopes embed `TenantCtx` directly (`ChannelMessage`, `OutMessage`, `OutboundEnvelope`). (Evidence: `libs/core/src/types.rs:12-239`; `libs/core/src/outbound.rs:1-25`; `docs/audit/_evidence/rg/tenant_context.txt`)

## Where tenant context is constructed

- `make_tenant_ctx` uses `GREENTIC_ENV` (default `dev`) plus tenant/team/user to build `TenantCtx`. (Evidence: `libs/core/src/context.rs:4-16`; `docs/audit/_evidence/rg/tenant_context.txt`)
- gsm-gateway constructs `TenantCtx` for each HTTP ingress using tenant + optional team + user id. (Evidence: `apps/messaging-gateway/src/http.rs:221-249`; `docs/audit/_evidence/rg/tenant_context.txt`)
- gsm-runner uses `InvocationEnvelope.ctx` as the tenant context for processing. (Evidence: `apps/runner/src/main.rs:216-224`; `docs/audit/_evidence/rg/tenant_context.txt`)

## Propagation and tagging across the stack

- Gateway embeds `TenantCtx` into `ChannelMessage` and publishes to NATS ingress subject with tenant/team in the subject path. (Evidence: `apps/messaging-gateway/src/http.rs:248-275`; `crates/gsm-bus/src/lib.rs:33-63`; `docs/audit/_evidence/rg/tenant_context.txt`)
- Runner converts `InvocationEnvelope` to `MessageEnvelope` (`handle_env`) and emits `OutMessage` in `run_one`. (Evidence: `apps/runner/src/main.rs:216-229`; `apps/runner/src/main.rs:140-176`; `docs/audit/_evidence/rg/tenant_context.txt`)
- Egress uses `OutMessage.ctx` and `OutMessage.tenant` to route and log; adapter selection uses platform. (Evidence: `apps/messaging-egress/src/main_logic.rs:112-189`; `docs/audit/_evidence/rg/tenant_context.txt`)

## Defaults, drops, and ignored context

- `make_tenant_ctx` defaults env to `dev` when `GREENTIC_ENV` is unset. (Evidence: `libs/core/src/context.rs:4-9`; `docs/audit/_evidence/rg/config.txt`)
- gsm-gateway defaults team to `MESSAGING_GATEWAY_DEFAULT_TEAM` when not provided; empty/blank team is sanitized. (Evidence: `apps/messaging-gateway/src/config.rs:53-54`; `apps/messaging-gateway/src/http.rs:221-226`; `docs/audit/_evidence/rg/tenant_context.txt`)
- `MessageEnvelope::into_invocation` sets `TenantCtx` with tenant + user only (team is `None`). (Evidence: `libs/core/src/types.rs:128-157`; `docs/audit/_evidence/rg/tenant_context.txt`)
- `MessageEnvelope::try_from(InvocationEnvelope)` overrides `tenant` and `user_id` fields from `InvocationEnvelope.ctx`. (Evidence: `libs/core/src/types.rs:161-199`; `docs/audit/_evidence/rg/tenant_context.txt`)

## Isolation: enforced vs tagged

- Enforced in routing: NATS subjects include tenant and team, which partitions ingress/egress traffic at the subject level. (Evidence: `crates/gsm-bus/src/lib.rs:33-63`; `libs/core/src/subjects.rs:20-49`; `docs/audit/_evidence/rg/tenant_context.txt`)
- Tagged in payloads: `TenantCtx` is stored in envelopes and used for logging/metrics, but not always used for routing decisions (e.g., gateway `OutMessage` uses `tenant` field + subject). (Evidence: `apps/messaging-gateway/src/http.rs:96-115`; `docs/audit/_evidence/rg/tenant_context.txt`)
- Provider registry keys include `EnvId`, `TenantId`, `TeamId` which enforce per-tenant provider instance caching. (Evidence: `libs/core/src/provider.rs:5-31`; `libs/core/src/registry.rs:23-44`; `docs/audit/_evidence/rg/tenant_context.txt`)
- Session store calls require `TenantCtx` and `UserId`, implying tenant/user-scoped session records. (Evidence: `libs/session/src/lib.rs:25-43`; `docs/audit/_evidence/rg/state_store.txt`)

## Known Unknowns

- Internal `TenantCtx` field layout and `greentic-types` validation logic are external to this repo; only usage sites are visible. (Evidence: `libs/core/src/prelude.rs:1-9`; `docs/audit/_evidence/cargo-metadata.json:1`)
