# Webex provider parity dossier (host-era vs WASM component)

Status: **initial draft (host-only; no WASM component found)**.

This dossier summarizes the Webex implementation in `greentic-messaging` and notes the absence of a corresponding WASM provider component in `../greentic-messaging-providers`.

## Summary of current behavior (host-era)

- **Formatting / rendering**: Webex renderer converts message-card IR into Adaptive Card 1.4 payloads, downgrades FactSets to text with warning `webex.factset_downgraded`, rejects inputs (`webex.inputs_not_supported`), enforces text limits (`WEBEX_TEXT_LIMIT`), and sanitizes URLs/text (`libs/core/src/messaging_card/renderers/webex.rs`).
- **Egress sender behavior**: `WebexSender` posts cards to Webex REST `messages` with bot token auth (`libs/core/src/platforms/webex/sender.rs`). Supports `mock://webex` base for tests, extracts returned `id`, and surfaces errors with retry hints for transient HTTP statuses.
- **Ingress service behavior**: `apps/ingress-webex` verifies webhook signatures (`X-Webex-Signature` with HMAC secret), provisions/refreshes webhooks when missing (`libs/core/src/platforms/webex/provision.rs`), and normalizes message/card/postback events via `apps/ingress-webex/src/normalise.rs`. Publishes normalized envelopes to NATS after idempotency checks (ingress-common).
- **Secrets schema expectations**: Host expects tenant/team-scoped `messaging/webex.credentials.json` containing `bot_token`, `webhook_secret`, and persisted webhook descriptors (`WebexCredentials` in `libs/core/src/platforms/webex/creds.rs`). Pack manifest documents the same key.
- **Capability metadata**: Declares Adaptive Card support, no threads, and rate limits in `providers/webex/provider.json`.

## Summary of current behavior (WASM component model)

- No Webex component or WIT world exists in `greentic-messaging-providers` today; host-only implementation.

## API/WIT gaps needed for behavioral parity

1. **Missing component** — No WASM contract exists; formatting, webhook validation, and provisioning are host-owned.
2. **Formatting richness** — Component WIT would need to accept full card IR (including actions and downgrades) and return Adaptive Card JSON plus warnings/limits to match `WebexRenderer`.
3. **Webhook normalization + signatures** — Component would need to parse Webex webhook envelopes (messages vs attachmentActions), apply signature verification with `webhook_secret`, and emit normalized events; current WIT shapes lack headers/signature inputs.
4. **Provisioning hints** — Host currently manages webhook lifecycle and persistence; decide whether a component should surface desired subscriptions/events or stay host-only.
5. **Secrets mapping** — Need a plan to pass `bot_token` and `webhook_secret` per tenant into the component; current WIT only models single-string secrets.

## Tests to preserve behavior

- Renderer snapshots in `libs/core/tests/renderers_webex_snapshot.rs` + fixtures under `libs/core/tests/fixtures/renderers/webex/`.
- Ingress normalization/unit tests in `apps/ingress-webex/src/normalise.rs` and `apps/ingress-webex/src/main.rs` modules.
- Egress sender tests covering `mock://` base and routing in `libs/core/src/platforms/webex/sender.rs`.
- Contract/e2e tests under `apps/egress-webex/tests/e2e_webex.rs`.

## Step-by-step migration considerations

1. Keep webhook HTTP server, signature verification, and provisioning in host; consider exposing a WASM `normalize_webhook` that returns the same shape as `WebexInboundEvent`.
2. Add a WASM `format-message` that consumes message-card IR and returns Adaptive Card JSON plus warnings/limits identical to `WebexRenderer`.
3. Define secrets contract (`WEBEX_BOT_TOKEN`, `WEBEX_WEBHOOK_SECRET`) and per-invocation injection strategy to align with tenant/team scoping.
4. Mirror existing tests as golden fixtures for any WASM outputs to ensure parity with host renderer and ingress normalization.
