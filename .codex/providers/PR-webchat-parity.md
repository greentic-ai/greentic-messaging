# WebChat provider parity dossier (host-era vs WASM component)

Status: **initial draft (host-only; no WASM component found)**.

This dossier summarizes the current WebChat implementation in `greentic-messaging` and notes the absence of a WASM provider component in `../greentic-messaging-providers`.

## Summary of current behavior (host-era)

- **Formatting / rendering**: WebChat renderer outputs Adaptive Card payloads from the full message-card IR (`libs/core/src/messaging_card/renderers/webchat.rs`). OAuth cards render as `application/vnd.microsoft.card.oauth` with `signin` buttons when present.
- **Ingress + runtime**: WebChat is a Direct Line–compatible gateway with two modes (proxy to Microsoft Direct Line via `webchat/channel_token`, or standalone in-memory store). The HTTP stack (token minting, conversation/session store, CORS, proactive admin API, OAuth callbacks) lives under `libs/core/src/platforms/webchat/http.rs`, `libs/core/src/platforms/webchat/standalone.rs`, and `libs/core/src/platforms/webchat/oauth.rs`. Tokens are HMAC-signed with `webchat/jwt_signing_key`; optional persistence is feature-flagged.
- **Egress**: `apps/egress-webchat/src/main.rs` consumes JetStream and fans out rendered cards/text to browser SSE subscribers keyed by `(env, tenant, team, user)`. Includes OAuth helper wiring for auth cards and DLQ/backpressure handling.
- **Secrets schema**: Requires `webchat/jwt_signing_key` (global) and optionally `webchat/channel_token` for proxy mode; OAuth routes consume `webchat_oauth/*` secrets (issuer, client_id, redirect_base, optional client_secret) documented in `providers/webchat/README.md`.
- **Capability metadata**: Declares `supports_threads: false`, Adaptive Card support, and rate limits in `providers/webchat/provider.json`.

## Summary of current behavior (WASM component model)

- No WebChat component or WIT world exists in `greentic-messaging-providers` today; host-only implementation.

## API/WIT gaps needed for behavioral parity

1. **Missing component** — No WIT or component code is available for WebChat; all functionality lives in host services.
2. **Direct Line surface area** — Any component would need to model Direct Line tokens, conversations, activities (including OAuth cards and proactive admin posts), and SSE/WebSocket event streaming, which is not expressible in the existing minimal WIT shape used by other providers.
3. **Secrets + signing** — Component API would need to accept JWT signing key and optional proxy channel token per tenant; current WIT patterns only support simple token injection.
4. **Ingress normalization** — Normalization today is effectively the Direct Line protocol itself; deciding what to delegate to WASM vs keep in host (token minting, session store, proactive endpoints) remains an open design call.

## Tests to preserve behavior

- Renderer snapshots cover WebChat card translation in `libs/core/tests/renderers_snapshot.rs`.
- End-to-end standalone/proactive behaviors are exercised in `libs/core/tests/webchat_proactive.rs` and helper scaffolding in `libs/core/tests/webchat_support.rs`.
- Manual/local instructions in `providers/webchat/README.md` and `libs/core/examples/run_standalone.rs` demonstrate expected routes and token flow.

## Step-by-step migration considerations

1. Decide if WebChat remains host-owned (recommended) vs a future WASM hybrid; if WASM is desired, design a WIT model that covers Direct Line activities/tokens rather than simple `(channel,text)` pairs.
2. Keep HTTP server, token minting, session store, OAuth, and proactive admin API in the host; only consider moving card rendering and optional activity normalization to WASM.
3. Extend component secrets contract to pass `webchat/jwt_signing_key` (global) and optional `webchat/channel_token` per invocation; ensure per-tenant scoping.
4. Add golden tests for any new WASM formatting/normalization outputs to match existing renderer and Direct Line expectations.
