# Web Chat Provider (Bot Framework Direct Line)

The `providers/webchat` crate exposes a Direct Line compatible façade for
Greentic Messaging. It now supports two deployment modes:

* **Standalone Direct Line** (feature: `directline_standalone`, enabled by
  default). All tokens, conversations, and websockets stay within Greentic.
* **Legacy proxy** (feature: `directline_proxy_ms`). The original PR-WC1
  behaviour that swaps Microsoft Direct Line secrets for short-lived tokens.

The documentation below focuses on the standalone setup. Notes about the legacy
proxy are included for completeness.

## Standalone Direct Line endpoints

| Method | Path | Description |
| ------ | ---- | ----------- |
| `POST` | `/v3/directline/tokens/generate?env=<env>&tenant=<tenant>[&team=<team>]` | Mint a short-lived Direct Line user token. |
| `POST` | `/v3/directline/conversations` | Requires `Authorization: Bearer <token>`; returns a conversation-bound token and optional `streamUrl`. |
| `GET` | `/v3/directline/conversations/:id/activities?watermark=<n>` | Poll for activities that occurred after watermark `n`. |
| `POST` | `/v3/directline/conversations/:id/activities` | Submit a user activity into the conversation. |
| `GET` | `/v3/directline/conversations/:id/stream?t=<conversation_token>&watermark=<n>` | WebSocket stream that delivers activities as they arrive. |

Additional endpoints under `/webchat` (OAuth, proactive admin API, health
checks) remain gated by the `webchat_bf_mode` feature and are unaffected by the
standalone mode.

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

Standalone mode requires an HS256 signing key:

```
WEBCHAT_JWT_SIGNING_KEY=local-dev-secret
```

Production deployments should source the key from the secrets backend
(`webchat/jwt/signing_key`). Tokens default to a 30 minute TTL. A simple
per-IP token bucket limits `/tokens/generate` to 5 requests per minute.
Manage the signing secret via `greentic-secrets`; the environment variable is
reserved for local development only.

### Tenant context

Tenant scope is supplied through the query parameters:

```
/v3/directline/tokens/generate?env=dev&tenant=acme&team=support
```

Tokens embed `{ ctx: { env, tenant, team? } }` and conversation access is
validated server-side.

## Legacy proxy mode (optional)

Enable `directline_proxy_ms` to keep calling Microsoft's Direct Line API. The
routes match the original `/webchat/:env/:tenant[:team]/tokens/generate` and
`/webchat/:env/:tenant[:team]/conversations/start` endpoints. Configure secrets
via `WEBCHAT_DIRECT_LINE_SECRET__{ENV}__{TENANT}[__{TEAM}]` (or legacy
`DL_SECRET__...`).

## OAuth and proactive messaging

OAuthCard support and the proactive admin API continue to live under the
`/webchat` prefix regardless of Direct Line mode. Refer to PR-WC4/PR-WC5 for
payload details — the configuration keys remain unchanged:

```
WEBCHAT_OAUTH_ISSUER__{ENV}__{TENANT}[__{TEAM}]
WEBCHAT_OAUTH_CLIENT_ID__{ENV}__{TENANT}[__{TEAM}]
WEBCHAT_OAUTH_REDIRECT_BASE__{ENV}__{TENANT}[__{TEAM}]
```

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
