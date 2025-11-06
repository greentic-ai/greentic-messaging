# WebChat Direct Line Gateway

The WebChat implementation now lives in the shared `gsm-core` crate under
`gsm_core::platforms::webchat`. This `providers/webchat` crate re-exports the
same types so existing call sites keep compiling, but new code should prefer the
core module directly.

Two complementary features are available:

* `directline_standalone` (default) — hosts a complete Direct Line service in
  process. Tokens and conversations are minted locally without reaching
  Microsoft's infrastructure.
* `directline_proxy_ms` — preserves the original proxy-to-Microsoft behaviour
  from PR-WC1. Enable it when you still need to exchange Microsoft's Direct Line
  secrets for short-lived tokens.

Both features are opt-in via Cargo feature flags. The default feature set is
`["webchat_bf_mode", "directline_standalone"]`.

## Standalone Direct Line (default)

Routes live at `/v3/directline` and are compatible with the Bot Framework
Direct Line protocol:

| Method | Path | Description |
| ------ | ---- | ----------- |
| `POST` | `/v3/directline/tokens/generate?env=<env>&tenant=<tenant>[&team=<team>]` | Mint a short-lived user token scoped to a `TenantCtx`. |
| `POST` | `/v3/directline/conversations` | Requires `Authorization: Bearer <token>`; returns a conversation-scoped token and stream URL. |
| `GET` | `/v3/directline/conversations/{id}/activities?watermark=<n>` | Poll activities pending for the conversation. |
| `POST` | `/v3/directline/conversations/{id}/activities` | Accept a user activity, mirror it into Greentic, and echo to subscribers. |
| `GET` | `/v3/directline/conversations/{id}/stream?t=<token>&watermark=<n>` | Bidirectional WebSocket that streams activities to Web Chat clients. |

Additional endpoints under `/webchat/...` (health checks, OAuth, proactive
admin API) remain available via the `webchat_bf_mode` feature.

### Key points

- **No Azure dependency**: all activities, watermarks, and websocket fan-out
  happen inside Greentic. The standalone Direct Line surface never calls
  Microsoft's Direct Line endpoints.
- **Token contents**: Direct Line JWTs encode the tenant context (`env`,
  `tenant`, optional `team`) in the `ctx` claim and may be bound to a specific
  conversation via the optional `conv` claim.
- **Persistence options**: enable `store_sqlite` or `store_redis` to persist
  conversations across restarts; otherwise the in-memory store is used.
- **Quotas & backpressure**: every conversation enforces a fixed backlog
  quota. When it is exceeded the server rejects new activities with HTTP 429.

### Configuration

All configuration now flows through an injected `Arc<dyn SecretsBackend>` which
is provided when constructing `WebChatProvider`. The provider looks up secrets
by scope using the following categories:

| Scope | Category / name | Purpose |
| ----- | ---------------- | ------- |
| Global (for example `secrets://global/webchat/_/webchat/jwt_signing_key`) | `webchat/jwt_signing_key` | HS256 signing key used by the standalone Direct Line server. |
| Tenant (`secrets://{env}/{tenant}/{team?}/webchat/channel_token`) | `webchat/channel_token` | Microsoft's Direct Line secret required when `directline_proxy_ms` is enabled. |
| Tenant (`secrets://{env}/{tenant}/{team?}/webchat_oauth/issuer`) | `webchat_oauth/issuer` | OAuth issuer URL. |
| Tenant (`secrets://{env}/{tenant}/{team?}/webchat_oauth/client_id`) | `webchat_oauth/client_id` | OAuth client identifier. |
| Tenant (`secrets://{env}/{tenant}/{team?}/webchat_oauth/redirect_base`) | `webchat_oauth/redirect_base` | Base URL used to build the OAuth callback. |
| Tenant (`secrets://{env}/{tenant}/{team?}/webchat_oauth/client_secret`, optional) | `webchat_oauth/client_secret` | Optional secret passed to the OAuth client. |

Tokens are valid for 30 minutes by default. `/v3/directline/tokens/generate`
is rate-limited per client IP (5 requests per minute).

### Embedding

```rust
use std::sync::Arc;
use gsm_core::platforms::webchat::{
    provider::WebChatProvider,
    config::Config,
    standalone::{StandaloneState, router as standalone_router},
};
use greentic_secrets::spec::{Scope, SecretsBackend};

let backend: Arc<dyn SecretsBackend + Send + Sync> = build_backend();
let signing_scope = Scope::new("global", "webchat", None)?;
let provider = WebChatProvider::new(Config::default(), backend).with_signing_scope(signing_scope);
let state = Arc::new(StandaloneState::new(provider).await?);
let app = standalone_router(Arc::clone(&state));
```

## Legacy proxy mode (optional)

To keep calling Microsoft's Direct Line service, compile with
`--features directline_proxy_ms`. The legacy routes remain unchanged:

* `POST /webchat/{env}/{tenant}[/{team}]/tokens/generate`
* `POST /webchat/{env}/{tenant}[/{team}]/conversations/start`
* `GET /webchat/healthz`

The provider retrieves the upstream Direct Line secret from the tenant scope's
`webchat/channel_token` entry before calling Microsoft.

## Local development

```bash
cargo test -p greentic-messaging-providers-webchat
```

The example app in `examples/webchat-demo/` connects directly to the
standalone endpoints. See its README for usage instructions.

### Running the standalone server locally

1. Provide a signing key via your secrets backend so the provider can resolve
   `webchat/jwt_signing_key`.
2. Start the example server: `cargo run --manifest-path providers/webchat/Cargo.toml --example run_standalone`.
   This binds the standalone Direct Line surface to `http://localhost:8090`.
   Point Web Chat at the standalone instance by configuring a Direct Line
   domain such as `https://localhost:8080/v3/directline`.

### Manual end-to-end test

1. Mint a user token:  
   `curl -s 'http://localhost:8090/v3/directline/tokens/generate?env=dev&tenant=acme' -X POST -H 'Content-Type: application/json' -d '{"user":{"id":"user-42"}}'`  
   Save the `token` value.
2. Create a conversation:  
   `curl -s http://localhost:8090/v3/directline/conversations -X POST -H "Authorization: Bearer ${USER_TOKEN}"`  
   Record the `conversationId`, conversation-scoped `token`, and `streamUrl`.
3. Connect a WebSocket client to the `streamUrl` (for example `websocat ${STREAM_URL}`) and leave it running.
4. Post a user activity:  
   `curl -s http://localhost:8090/v3/directline/conversations/${CONVERSATION_ID}/activities -X POST -H "Authorization: Bearer ${CONVERSATION_TOKEN}" -H 'Content-Type: application/json' -d '{"type":"message","text":"hello from manual test"}'`  
   Confirm the WebSocket receives the message and, if desired, poll `GET /v3/directline/conversations/${CONVERSATION_ID}/activities` to see the watermark advance.
5. Append a bot activity via the admin API:  
   `curl -s http://localhost:8090/webchat/admin/dev/acme/post-activity -X POST -H 'Content-Type: application/json' -d '{"conversation_id":"'"${CONVERSATION_ID}"'","activity":{"type":"message","text":"bot ack ✅"}}'`  
   Expect `{ "posted": 1, "skipped": 0 }` and observe the bot message on the WebSocket stream. Tests are complete when both user and bot messages appear and watermarks advance.
