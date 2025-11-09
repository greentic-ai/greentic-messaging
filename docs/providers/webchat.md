# Web Chat Provider (Bot Framework Direct Line)

The Direct Line implementation now ships under **`gsm-core::platforms::webchat`**.
The legacy `providers/webchat` crate has been collapsed to metadata (descriptor +
schemas); binaries and tests should depend on `gsm-core` directly.

Two operating modes are available:

- **Proxy (default build)** — WebChat forwards Direct Line calls to Microsoft
  using the tenant's `webchat/channel_token` secret.
- **Standalone Direct Line** — Enable the `directline_standalone` feature to keep
  all tokens, conversation state, and WebSocket fan-out inside Greentic. This is
  the mode referenced by the PR-WC2 through PR-WC7 prompts.

The sections below focus on the standalone server while noting where the proxy
behaviour differs.

## Standalone Direct Line endpoints

| Method | Path | Description |
| ------ | ---- | ----------- |
| `POST` | `/v3/directline/tokens/generate?env=<env>&tenant=<tenant>[&team=<team>]` | Mint a short-lived Direct Line user token. |
| `POST` | `/v3/directline/conversations` | Requires `Authorization: Bearer <token>`; returns a conversation-bound token and optional `streamUrl`. |
| `GET` | `/v3/directline/conversations/{id}/activities?watermark=<n>` | Poll for activities that occurred after watermark `n`. |
| `POST` | `/v3/directline/conversations/{id}/activities` | Submit a user activity into the conversation. |
| `GET` | `/v3/directline/conversations/{id}/stream?t=<conversation_token>&watermark=<n>` | WebSocket stream that delivers activities as they arrive. |

Additional endpoints under `/webchat` (OAuth, proactive admin API, health
checks) remain available in both modes.

### What stays local

- No Azure dependency: the standalone Direct Line server stores every activity,
  advances watermarks, and pushes updates to connected WebSocket clients without
  contacting Microsoft's infrastructure.
- Tokens embed `{ ctx: { env, tenant, team? } }` along with an optional
  conversation binding (`conv`). Conversation access checks compare the JWT
  context with the locally stored `TenantCtx`.
- Persistence options: enable `store_sqlite` or `store_redis` to keep
  conversations across restarts; otherwise an in-memory store is used.
- Backpressure: per-conversation quotas cap backlog size. The server returns
  HTTP 429 once the quota is exceeded.

### Request flow

1. Call `POST /v3/directline/tokens/generate?...` to obtain a user token.
2. Call `POST /v3/directline/conversations` with `Authorization: Bearer <user token>`.
   The response contains a conversation-scoped token and optional `streamUrl` for WS clients.
3. Use the conversation token with the Direct Line REST/WS APIs (e.g. Bot Framework Web Chat's `createDirectLine({ token, domain: "https://host/v3/directline" })`).

### Configuration

Standalone mode resolves all secrets via an injected `Arc<dyn SecretsBackend>`.
At minimum the following entries must exist:

| Scope | Category / name | Purpose |
| ----- | ---------------- | ------- |
| Global signing scope (for example `secrets://global/webchat/_/webchat/jwt_signing_key`) | `webchat/jwt_signing_key` | HS256 key used to mint Direct Line tokens. |
| Tenant scope (`secrets://{env}/{tenant}/{team?}/webchat/channel_token`) | `webchat/channel_token` | Microsoft's Direct Line secret, only required in proxy mode. |
| Tenant scope (`secrets://{env}/{tenant}/{team?}/webchat_oauth/issuer`) | `webchat_oauth/issuer` | OAuth issuer URL used by the `/webchat/oauth/...` routes. |
| Tenant scope (`secrets://{env}/{tenant}/{team?}/webchat_oauth/client_id`) | `webchat_oauth/client_id` | OAuth client identifier. |
| Tenant scope (`secrets://{env}/{tenant}/{team?}/webchat_oauth/redirect_base`) | `webchat_oauth/redirect_base` | Base URL used to build the OAuth callback URL. |
| Tenant scope (`secrets://{env}/{tenant}/{team?}/webchat_oauth/client_secret`, optional) | `webchat_oauth/client_secret` | Optional client secret forwarded to `GreenticOauthClient`. |

Inject the backend when constructing `WebChatProvider` (from `gsm_core`) and
hand it to `AppState`/`StandaloneState`:

```rust
use std::sync::Arc;
use gsm_core::platforms::webchat::{
    standalone_router, WebChatProvider, Config, StandaloneState,
};
use greentic_secrets::spec::Scope;

let backend: Arc<dyn greentic_secrets::spec::SecretsBackend + Send + Sync> = build_backend();
let signing_scope = Scope::new("global", "webchat", None)?;
let provider = WebChatProvider::new(Config::default(), backend).with_signing_scope(signing_scope);
let state = Arc::new(StandaloneState::new(provider).await?);
let app = standalone_router(Arc::clone(&state));
```

Tokens default to a 30 minute TTL. `/v3/directline/tokens/generate` is rate-limited
to 5 requests per minute per client IP.

### Tenant context

Tenant scope is supplied through the query parameters:

```
/v3/directline/tokens/generate?env=dev&tenant=acme&team=support
```

Tokens embed `{ ctx: { env, tenant, team? } }` and conversation access is
validated server-side.

## Microsoft Direct Line proxy (default build)

Without the `directline_standalone` feature the provider proxies straight to
Microsoft's Direct Line endpoints. The legacy
`/webchat/{env}/{tenant}[/{team}]/tokens/generate` and
`/webchat/{env}/{tenant}[/{team}]/conversations/start` routes remain available in
addition to the `/v3/directline/**` surface. The tenant's
`webchat/channel_token` secret is forwarded as the upstream bearer token.

## OAuth and proactive messaging

OAuthCard support and the proactive admin API continue to live under the
`/webchat` prefix regardless of Direct Line mode. Refer to PR-WC4/PR-WC5 for
payload details. The provider resolves OAuth configuration from the tenant
scope using the `webchat_oauth/*` secrets listed above.

When WebChat egress receives an Adaptive `kind="oauth"` card it contacts the
shared greentic-oauth service (export `OAUTH_BASE_URL`) to mint a signed
`start_url` and to look up the Bot Framework `connectionName`. Missing presets
trigger the downgrade heuristics described in the repository README, so the
conversation still receives a clickable fallback.

## Demo application

`examples/webchat-demo/` uses the standalone Direct Line endpoints. Run the
provider locally, then:

```bash
cd examples/webchat-demo
npm install
npm run dev
```

The Vite dev server proxies `/v3/directline` to `http://localhost:8090`. Tweak
`VITE_WEBCHAT_*` variables in `.env.local` to target different environments.
Specify a Direct Line domain (defaults to `https://localhost:8080/v3/directline`)
using `VITE_WEBCHAT_DIRECTLINE_DOMAIN`.

The demo walks through token generation, conversation creation, WebSocket
streaming, Adaptive Cards, OAuth, and proactive messaging scenarios.

## Conformance test suite

See [`conformance/webchat`](../conformance/webchat/) for automated coverage.
The suite exercises the standalone endpoints and runs in CI on every pull
request and nightly schedule.
