# Repository Overview

## 1. High-Level Purpose
- Serverless-ready messaging runtime that normalizes ingress traffic from multiple chat platforms (Slack, Teams, Telegram, WebChat, Webex, WhatsApp), routes it over NATS/JetStream, and fans it back out through egress workers with rate limiting, DLQ, and idempotency.
- Provides a shared MessageCard engine (rendering/downgrades/telemetry) and translation layer so platform-agnostic flows can produce provider-specific payloads. Includes orchestration CLI, flow runner, dev tooling, and conformance/golden fixtures.

## 2. Main Components and Functionality
- **Path:** `libs/core`
  **Role:** Shared contracts, platform enums, message envelopes, subject naming, secrets paths, and MessageCard engine (renderers, downgrade policies, telemetry). Hosts Direct Line–style WebChat provider and HTTP helpers.
  **Key functionality:** Tenant-aware contexts, ingress/egress types, OAuth helpers, adapter registry, validation utilities, Adaptive Card rendering/downgrades per platform, WebChat standalone server, subject helpers for NATS routing.
- **Path:** `libs/translator`
  **Role:** Translates platform-agnostic `OutMessage`/MessageCard payloads into provider-specific JSON.
  **Key functionality:** Secure action URL generation, card rendering via shared engine, per-platform translators (`slack`, `teams`, `telegram`, `webex`) emitting ready-to-send payloads.
- **Path:** `libs/security`
  **Role:** Security utilities for hashing state, JWT signing/verification for actions, link construction, nonce middleware.
  **Key functionality:** Action claims signer, URL builders, middleware to validate signed action invocations.
- **Path:** `libs/telemetry`
  **Role:** Lightweight wrapper over `greentic-telemetry` to install tracing/metrics and attach message/tenant context.
  **Key functionality:** Telemetry context helpers, auth card metrics, metric recorders, dev-friendly JSON logging toggle.
- **Path:** `libs/session`
  **Role:** Shared session storage wrapper over greentic-session backends.
  **Key functionality:** In-memory or Redis store selection from env, async helpers to find/create/update sessions keyed by tenant/user.
- **Path:** `libs/idempotency`
  **Role:** Idempotency guard built on JetStream key-value.
  **Key functionality:** Claim/store request IDs to prevent duplicate ingress processing.
- **Path:** `libs/backpressure`
  **Role:** Distributed rate limiting using JetStream KV with local token buckets.
  **Key functionality:** Rate limit config from env, hybrid limiter that records telemetry gauges and throttles per-tenant sends.
- **Path:** `libs/dlq`
  **Role:** Dead-letter queue publisher on NATS JetStream with replay subject helpers.
  **Key functionality:** Streams DLQ entries with metadata, publishes telemetry counters, exposes replay subscription subject names.
- **Path:** `libs/gsm-provider-registry`
  **Role:** Registry/manifest loader for adapters and outbox management.
  **Key functionality:** Provider manifest parsing, builder hooks, outbox idempotency keys, adapter traits for send/receive pipelines.
- **Path:** `libs/telemetry`, `libs/testutil`, `libs/backpressure`, `libs/dlq`, `libs/idempotency`, `libs/security`
  **Role:** Supporting utilities for telemetry, testing fixtures/mocks, rate limiting, DLQ, idempotency, and security primitives shared across binaries.
- **Path:** `apps/ingress-common`
  **Role:** Shared middleware, telemetry, and rate limiting used by ingress services.
- **Path:** `apps/egress-common`
  **Role:** Shared JetStream consumer bootstrap and telemetry helpers for egress workers.
- **Path:** `apps/messaging-gateway`
  **Role:** Pack-aware HTTP ingress gateway that normalizes inbound traffic and publishes to NATS subjects.
- **Path:** `apps/messaging-egress`
  **Role:** Pack-aware JetStream worker that invokes greentic-runner and publishes outbound envelopes.
- **Path:** `legacy/apps/*`
  **Role:** Per-platform ingress/egress binaries retained for migration (Slack/Teams/Telegram/WebChat/Webex/WhatsApp).
- **Path:** `apps/runner`
  **Role:** Flow orchestrator that loads YAML-defined flows, executes QA/tool/template/card nodes, maintains per-user session state, and emits `OutMessage` to NATS out-subjects.
  **Key functionality:** Handlebars templating, tool execution, session persistence via greentic-session, DLQ replay handling, auth card telemetry.
- **Path:** `apps/subscriptions-teams`
  **Role:** Manages Microsoft Teams webhook subscriptions, publishes incoming events to NATS with tenant context.
- **Path:** `apps/cli-dlq`
  **Role:** CLI utility to interact with DLQ entries (consume/inspect) using shared DLQ helpers.
- **Path:** `apps/slack_oauth`
  **Role:** OAuth helper service for Slack installations, storing workspace credentials under tenant/team scopes.
- **Path:** `crates/greentic-messaging-cli`
  **Role:** Developer/operator CLI that wraps Make targets to bring up ingress/egress/subscription services, inspect env/secrets, run flows, and drive adapter tests/guard-rails.
  **Key functionality:** Discovers tenants/teams from secrets, forwards commands to cargo binaries, provides admin helpers for Slack/Teams/Telegram/WhatsApp setup.
- **Path:** `crates/messaging-test`
  **Role:** Fixture-driven test harness for adapters/translators with CLI commands to list adapters, run fixtures, and generate golden artifacts.
- **Path:** `tools/dev-viewer`, `tools/nats-demo`, `tools/mock-weather-tool`, `conformance/webchat`
  **Role:** Dev and demo tooling—MessageCard preview web UI, NATS demo producer, mock weather tool for flows, and WebChat conformance tests/fixtures.
  **Notes:** `examples/flows` and `libs/cards` house sample flows and card fixtures; docs under `docs/` cover telemetry/secrets and CLI usage.

## 3. Work In Progress, TODOs, and Stubs
- **Location:** libs/core/src/runner_client.rs (LoggingRunnerClient)
  **Status:** Intentional stub for dev/tests
  **Short description:** Logging-only runner client kept for local/test use; production paths should use HttpRunnerClient or a real runner.
- **Location:** crates/messaging-test (packs validation)
  **Status:** Pack validator now materializes components via greentic-distributor-client by default; additional polish/maintenance may follow.
  **Short description:** Packs dry-run validator fetches/ resolves public OCI components (offline/allow-tag toggles) before linting flows to keep validation runner-free.

## 4. Broken, Failing, or Conflicting Areas
- **Location:** None
  **Evidence:** `cargo test --workspace` passes in this checkout.
  **Likely cause / nature of issue:** N/A.

## 5. Notes for Future Work
- Provider migration to WASM components is tracked via parity dossiers under `.codex/providers/PROVIDER_PARITY_INDEX.md`.
- Added pack-backed fixture execution to `greentic-messaging-test` run/all when `--pack` is provided, invoking adapters via greentic-runner.
- New CLI flags for pack-backed runs: `--pack`, `--packs-root`, `--runner-url`, `--runner-api-key`, `--env`, `--tenant`, `--team`, `--chat-id`, `--platform`.
