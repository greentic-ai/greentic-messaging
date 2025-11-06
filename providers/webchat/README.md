# WebChat Direct Line Gateway

WebChat's Direct Line runtime now ships from `gsm-core::platforms::webchat`. The
`providers/webchat` directory is kept for registry metadata (`provider.json`)
and documentation only—no Rust crate is built from here.

## Modes & features

- **Proxy (default build)** — forwards Direct Line calls to Microsoft using the
  tenant-scoped `webchat/channel_token` secret.
- **Standalone Direct Line** (`directline_standalone`) — runs a self-contained
  Direct Line server inside Greentic with no Azure dependency. This is the
  default for local demos and the WC2–WC7 prompts.
- **Persistence** (`store_sqlite`, `store_redis`) — optional features that keep
  conversations across restarts; otherwise an in-memory store is used.

Additional `/webchat/...` routes (OAuth, proactive admin API, health checks)
remain available in both modes.

### Configuration

All configuration flows through an injected `Arc<dyn SecretsBackend>` provided
when constructing `WebChatProvider`. Secrets are resolved per scope:

| Scope | Category / name | Purpose |
| ----- | ---------------- | ------- |
| Global (for example `secrets://global/webchat/_/webchat/jwt_signing_key`) | `webchat/jwt_signing_key` | HS256 signing key used to mint Direct Line tokens. |
| Tenant (`secrets://{env}/{tenant}/{team?}/webchat/channel_token`) | `webchat/channel_token` | Required when proxying to Microsoft's Direct Line API. |
| Tenant (`secrets://{env}/{tenant}/{team?}/webchat_oauth/issuer`) | `webchat_oauth/issuer` | OAuth issuer URL for `/webchat/oauth/...`. |
| Tenant (`secrets://{env}/{tenant}/{team?}/webchat_oauth/client_id`) | `webchat_oauth/client_id` | OAuth client identifier. |
| Tenant (`secrets://{env}/{tenant}/{team?}/webchat_oauth/redirect_base`) | `webchat_oauth/redirect_base` | Base URL used to build the OAuth callback. |
| Tenant (`secrets://{env}/{tenant}/{team?}/webchat_oauth/client_secret`, optional) | `webchat_oauth/client_secret` | Optional secret forwarded to the OAuth client. |

Direct Line tokens default to a 30‑minute TTL. `/v3/directline/tokens/generate`
is rate-limited to 5 requests per minute per client IP.

### Embedding

```rust
use std::sync::Arc;
use gsm_core::platforms::webchat::{Config, StandaloneState, WebChatProvider, standalone_router};
use greentic_secrets::spec::{Scope, SecretsBackend};

let backend: Arc<dyn SecretsBackend + Send + Sync> = build_backend();
let signing_scope = Scope::new("global", "webchat", None)?;
let provider = WebChatProvider::new(Config::default(), backend).with_signing_scope(signing_scope);
let state = Arc::new(StandaloneState::new(provider).await?);
let app = standalone_router(Arc::clone(&state));
```

## Local development

```bash
cargo test -p gsm-core --features directline_standalone
```

Add `store_sqlite` and/or `store_redis` to exercise the persistence backends.
`cargo clippy -p gsm-core --all-features` ensures the optional code paths stay
clean.

The demo app in `examples/webchat-demo/` points at the standalone endpoints. See
its README for usage.

### Running the standalone server locally

1. Provide a signing key so the secrets backend can resolve
   `webchat/jwt_signing_key`. The example server accepts
   `WEBCHAT_JWT_SIGNING_KEY` for convenience.
2. Start the example:  
   `cargo run --manifest-path libs/core/Cargo.toml --example run_standalone`  
   The server listens on `http://localhost:8090` and exposes `/v3/directline/**`.
3. Configure Web Chat (or the demo app) with `domain: "https://localhost:8080/v3/directline"`
   when running through a local HTTPS terminator.

### Manual end-to-end test

1. Mint a user token:  
   `curl -s 'http://localhost:8090/v3/directline/tokens/generate?env=dev&tenant=acme' -X POST -H 'Content-Type: application/json' -d '{"user":{"id":"user-42"}}'`  
   Save the `token` field.
2. Create a conversation:  
   `curl -s http://localhost:8090/v3/directline/conversations -X POST -H "Authorization: Bearer ${USER_TOKEN}"`  
   Record `conversationId`, the conversation `token`, and `streamUrl`.
3. Connect a WebSocket client to `${STREAM_URL}` (for example `websocat ${STREAM_URL}`) and leave it running.
4. Post a user activity:  
   `curl -s http://localhost:8090/v3/directline/conversations/${CONVERSATION_ID}/activities -X POST -H "Authorization: Bearer ${CONVERSATION_TOKEN}" -H 'Content-Type: application/json' -d '{"type":"message","text":"hello from manual test"}'`  
   Confirm the WebSocket receives the message; optionally poll
   `GET /v3/directline/conversations/${CONVERSATION_ID}/activities` to see the
   watermark advance.
5. Append a bot activity via the admin API:  
   `curl -s http://localhost:8090/webchat/admin/dev/acme/post-activity -X POST -H 'Content-Type: application/json' -d '{"conversation_id":"'"${CONVERSATION_ID}"'","activity":{"type":"message","text":"bot ack ✅"}}'`  
   Expect `{ "posted": 1, "skipped": 0 }` and observe the bot message on the
   WebSocket stream. The manual test passes once both user and bot messages flow
   through and watermarks advance.
