# 05-providers-sources-sinks

## Provider abstractions (core)

- `Provider` trait (marker) + `ProviderRegistry` keyed by `ProviderKey` (tenant/env/team/platform). (Evidence: `libs/core/src/registry.rs:5-44`; `libs/core/src/provider.rs:5-31`; `docs/audit/_evidence/rg/providers.txt`)
- `PlatformProvider` trait defines `send_card`, `verify_webhook`, optional `raw_call`, and a `PlatformInit` bundle (secrets/telemetry/http/card renderer). (Evidence: `libs/core/src/platforms/provider.rs:9-52`; `docs/audit/_evidence/rg/providers.txt`)
- WebChat provider implements `PlatformProvider` but `send_card`/`verify_webhook` are `bail!` (explicit not implemented). (Evidence: `libs/core/src/platforms/webchat/provider.rs:119-150`; `docs/audit/_evidence/rg/overlaps_wip.txt`)

## Provider abstractions (gsm-provider-registry)

- Manifest-driven registry of send/receive adapters (`ProviderRegistry`, `ProviderHandles`). (Evidence: `libs/gsm-provider-registry/src/registry.rs:11-107`; `docs/audit/_evidence/rg/providers.txt`)
- Adapter traits: `SendAdapter` and `ReceiveAdapter` operate on `TenantCtx` + message payload. (Evidence: `libs/gsm-provider-registry/src/traits.rs:34-41`; `docs/audit/_evidence/rg/tenant_context.txt`)
- Built-in providers are registered per platform (e.g., `providers/slack`, `providers/teams`, `providers/webchat`, etc.). (Evidence: `libs/gsm-provider-registry/src/providers/slack/mod.rs:19-44`; `docs/audit/_evidence/rg/providers.txt`)

## Pack-driven provider extensions

- Pack extensions registry (`ProviderExtensionsRegistry`) supports ingress, OAuth, and subscriptions provider declarations. (Evidence: `libs/core/src/pack_extensions.rs:60-115`; `docs/audit/_evidence/rg/providers.txt`)
- Extensions are loaded from `.gtpack` or YAML pack specs and merged into registries. (Evidence: `libs/core/src/pack_extensions.rs:101-159`; `docs/audit/_evidence/rg/providers.txt`)

## Adapter registry (packs)

- `AdapterDescriptor` and `AdapterRegistry` are populated from pack YAML or `.gtpack` manifests and used by gateway/egress. (Evidence: `libs/core/src/adapter_registry.rs:24-118`; `docs/audit/_evidence/rg/providers.txt`)
- `.gtpack` loading recovers provider extensions from `manifest.cbor` for compatibility. (Evidence: `libs/core/src/adapter_registry.rs:158-188`; `docs/audit/_evidence/rg/overlaps_wip.txt`)

## Sources / sinks and invocation transports

- `RunnerClient` abstraction supports `invoke_adapter` with `OutMessage` and `AdapterDescriptor`. (Evidence: `libs/core/src/runner_client.rs:10-17`; `docs/audit/_evidence/rg/envelopes.txt`)
- `LoggingRunnerClient` is a stub; `HttpRunnerClient` POSTs JSON to an external runner endpoint with optional `Authorization` header. (Evidence: `libs/core/src/runner_client.rs:19-85`; `docs/audit/_evidence/rg/providers.txt`)
- Gateway worker routing uses `WorkerClient` with NATS or HTTP transport; worker requests embed tenant context and payload metadata. (Evidence: `libs/core/src/worker.rs:20-200`; `docs/audit/_evidence/rg/tenant_context.txt`)

## HTTP request construction

- `HttpRunnerClient` builds JSON payloads for adapter invocation using reqwest. (Evidence: `libs/core/src/runner_client.rs:61-85`; `docs/audit/_evidence/rg/providers.txt`)
- Slack OAuth helper builds HTTP requests to Slack API via reqwest and stores results via secrets resolver. (Evidence: `apps/slack_oauth/src/main.rs:90-201`; `docs/audit/_evidence/rg/secrets.txt`)

## Known Unknowns

- Concrete provider implementations in external crates or WASM components referenced by adapter packs are not visible here; only registry/manifest wiring is in-repo. (Evidence: `libs/core/src/adapter_registry.rs:143-188`; `docs/audit/_evidence/cargo-metadata.json:1`)
