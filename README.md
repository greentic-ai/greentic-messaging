# greentic_messaging
Serverless-ready messaging runtime for multi-platform chat, with NATS routing and MessageCard translation.
This repo contains:
- apps/: ingress/egress/runner/subscriptions services
- libs/: shared crates (core types, translators, security)
- examples/: flows and payloads

## Build
```bash
cargo build
```
## Test
```bash
cargo test
cargo test -p gsm-runner --features chaos -- --ignored chaos
```

## Test Coverage

Automated coverage runs in CI through `cargo-tarpaulin`; every push and pull
request uploads an LCOV report as a build artifact. To reproduce the numbers
locally:

```bash
cargo install cargo-tarpaulin
cargo tarpaulin --workspace --all-features --out Lcov --output-dir coverage
```

The generated `coverage/` directory contains the LCOV output that mirrors the
artifact uploaded by GitHub Actions.

## Environment & Tenant Context

- `GREENTIC_ENV` selects the environment scope for a deployment (`dev`, `test`, `prod`). When unset, the runtime defaults to `dev` so local flows continue to work without extra configuration.
- Every ingress normalises requests into an `InvocationEnvelope` carrying a full `TenantCtx` (`env`, `tenant`, optional `team`/`user`, tracing metadata). Downstream services (runner, egress) now receive the same shape regardless of source platform.
- Export `TENANT` (and optionally `TEAM`) alongside channel-specific secrets when running locally so worker subjects resolve correctly.
- Secrets resolvers and egress senders consume the shared context, making it safe to host multiple environments or teams within a single process.
- Provider credentials live under `secret://{env}/{tenant}/{team|default}/messaging/{platform}-{team|default}-credentials.json`, so each environment stays isolated by design.

## Sending Messages

All egress adapters now share a common interface: pass a `TenantCtx` alongside an
`OutboundMessage` and the sender selects the scoped credentials automatically.

```rust
use std::sync::Arc;

use gsm_core::egress::{OutboundMessage, SendResult};
use gsm_core::platforms::slack::sender::SlackSender;
use gsm_core::prelude::{make_tenant_ctx, DefaultResolver};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // GREENTIC_ENV defaults to "dev" but can be overridden for other scopes.
    std::env::set_var("GREENTIC_ENV", "dev");

    let ctx = make_tenant_ctx("acme".into(), Some("support".into()), None);
    let resolver = Arc::new(DefaultResolver::new().await?);
    let sender = SlackSender::new(reqwest::Client::new(), resolver, None);

    let msg = OutboundMessage {
        channel: Some("C123".into()),
        text: Some("Hello from Greentic".into()),
        payload: None,
    };

    let SendResult { message_id, .. } = sender.send(&ctx, msg).await?;
    println!("posted message id: {:?}", message_id);
    Ok(())
}
```

For testing, each egress binary exposes a `mock-http` Cargo feature that swaps
remote API calls for in-memory mocks (`cargo test -p gsm-egress-slack --features
mock-http`).

## Slack Integration

1. Create a Slack app with a Bot User token (Scopes: `app_mentions:read`, `channels:history`, `groups:history`, `im:history`, `mpim:history`, `chat:write`, `commands`).
2. Run the OAuth helper to install the app per tenant/team:
   ```bash
   export SLACK_CLIENT_ID=...
   export SLACK_CLIENT_SECRET=...
   export SLACK_REDIRECT_URI=https://<public-host>/slack/callback
   export SLACK_SCOPES="app_mentions:read,channels:history,chat:write"
   export SLACK_SIGNING_SECRET=...
   cargo run -p gsm-slack-oauth
   ```
   Visit `/slack/install?tenant=acme&team=support` to initiate an install. When Slack redirects back to `/slack/callback`, the handler stores the workspace credentials at `/{env}/messaging/slack/{tenant}/{team}/workspace/<team_id>.json` (the runtime defaults to `env=dev`).
3. Start the services:
   ```bash
   make stack-up             # optional: start local nats/docker stack
   make run-ingress-slack
   make run-egress-slack
   FLOW=examples/flows/weather_slack.yaml PLATFORM=slack make run-runner
   ```
4. Point Slack Event Subscriptions to `/ingress/slack/{tenant}/{team}/events`. The ingress verifies the signing secret, emits an `InvocationEnvelope`, and publishes to NATS. Replies with a `thread_ts` keep the conversation threaded.

## Microsoft Teams Integration

1. Register an Azure AD app with permissions to Microsoft Graph (`Chat.ReadWrite`, `ChannelMessage.Send`, `ChatMessage.Send`) and create a client secret. Configure a subscription webhook pointing to `/teams/webhook`.
2. Export required values:
   ```bash
   export MS_GRAPH_TENANT_ID=...
   export MS_GRAPH_CLIENT_ID=...
   export MS_GRAPH_CLIENT_SECRET=...
   export TEAMS_WEBHOOK_URL=https://<public>/teams/webhook
   export TENANT=acme
   export NATS_URL=nats://127.0.0.1:4222
   ```
3. Start the services (ingress, egress, subscription manager):
   ```bash
   make run-ingress-teams
   make run-egress-teams
   make run-subscriptions-teams
   FLOW=examples/flows/weather_telegram.yaml PLATFORM=teams make run-runner
   ```
4. Add the Graph subscription through the admin subject (`greentic.subs.admin`) or use the runner to trigger messages; cards are translated into Adaptive Cards for Teams.

## Admin & Security Helpers

Optional guard rails apply to all ingress services (Telegram, Slack, etc.) through `apps/ingress-common/src/security.rs`. Leave them unset for local dev, or export them and supply matching headers when you need protection.

- `INGRESS_BEARER`: when set, requests must include `Authorization: Bearer $INGRESS_BEARER`.
- `INGRESS_HMAC_SECRET`: enable HMAC validation for webhook/admin calls; compute base64(hmac_sha256(secret, body)) and send it in `INGRESS_HMAC_HEADER` (defaults to `x-signature`).
- `INGRESS_HMAC_HEADER`: override the signature header name.

Action Links (optional): provide `JWT_SECRET`, `JWT_ALG` (e.g. HS256), and `ACTION_BASE_URL` so ingress can generate signed deeplinks for card actions. Missing JWT envs just disable the feature (you’ll see a log warning).

Admin endpoints share the same middleware stack as `/telegram/webhook`. If guards are enabled, include the headers when curling (example below). Otherwise, the endpoints are open on localhost.

Example status call with bearer + HMAC:
```bash
sig=$(printf '' | openssl dgst -binary -sha256 -hmac "$INGRESS_HMAC_SECRET" | base64)
curl -s \
  -H "Authorization: Bearer $INGRESS_BEARER" \
  -H "${INGRESS_HMAC_HEADER:-x-signature}: $sig" \
  http://localhost:8080/admin/telegram/acme/status | jq
```


## Telegram Integration

1. Create a Telegram bot via BotFather and obtain the bot token; configure the webhook secret if desired.
2. Export environment variables:
   ```bash
   export TELEGRAM_SECRET_TOKEN=dev
   export TENANT=acme
   # Optional: point to a tenants.yaml file if you manage multiple tenants
   # export TENANT_CONFIG=config/tenants.yaml
   # TELEGRAM_PUBLIC_WEBHOOK_BASE will fall back to http://localhost:8080/telegram/webhook
   export TELEGRAM_PUBLIC_WEBHOOK_BASE=https://gsm.greentic.ai/telegram/webhook
   export NATS_URL=nats://127.0.0.1:4222
   ```
3. Start ingress, egress, and the runner:
```bash
make run-ingress-telegram
make run-egress-telegram
FLOW=examples/flows/weather_telegram.yaml PLATFORM=telegram make run-runner
```
4. Set the Telegram webhook to point at `/telegram/webhook`. Messages sent to the bot are normalized, routed through NATS, and responses are delivered via the Telegram egress adapter using the official Bot API.

**Tenant configuration**

- `TENANT` identifies the default tenant for single-tenant deployments and is used when subscribing to inbound subjects.
- `TENANT_CONFIG` (optional) points at a YAML file describing tenants and their Telegram settings. When omitted, the service synthesizes a single-tenant configuration from the environment variables above.

**Secrets & tenants**

- Secrets default to environment variables named `TENANTS_<TENANT>_TELEGRAM_BOT_TOKEN` and `TENANTS_<TENANT>_TELEGRAM_SECRET_TOKEN` (uppercase, slashes replaced with `_`). Provide these or configure `TENANT_CONFIG` to source them elsewhere.

**Startup reconciliation & admin endpoints**

- On boot the ingress service reconciles Telegram webhooks for every enabled tenant and emits `greentic_telegram_webhook_reconciles_total{tenant,result}` metrics.
- The admin API exposes helpers for CI/ops:
  - `POST /admin/telegram/{tenant}/register`
  - `POST /admin/telegram/{tenant}/deregister`
  - `GET  /admin/telegram/{tenant}/status`
- Bootstrap secrets before enabling a tenant: store `tenants/<tenant>/telegram/bot_token` and `tenants/<tenant>/telegram/secret_token` (or allow the service to generate the secret if your store supports writes).
- First-time registration sets `drop_pending_updates=true`; the admin `register` endpoint keeps history by default (`drop_pending_updates=false`).

## WebChat Integration

1. Embed the widget script on any page and provide tenant context via data attributes:
   ```html
   <script src="https://<your-ingress>/widget.js" data-tenant="acme" data-team="support" data-env="dev"></script>
   ```
   The widget automatically includes these values in `channelData` when posting to the ingress.
2. Export the runtime variables (egress still uses `TENANT` to select the outbound subject):
   ```bash
   export NATS_URL=nats://127.0.0.1:4222
   export TENANT=acme
   ```
3. Launch the services:
   ```bash
   make run-ingress-webchat
   make run-egress-webchat
   FLOW=examples/flows/weather_slack.yaml PLATFORM=webchat make run-runner
   ```
4. POST inbound messages to `/webhook` with camelCase keys:
   ```json
   {
     "chatId": "chat-1",
     "userId": "user-42",
     "text": "Hello",
     "channelData": {
       "tenant": "acme",
       "team": "support",
       "env": "dev"
     }
   }
   ```
   Tenant is required; team is optional. Open `http://localhost:8090` to try the sample form that ships with the ingress.

## WhatsApp Integration

1. Create a WhatsApp Business App, obtain the phone number ID, and generate a permanent user access token. Pick a webhook verify token and note the app secret.
2. Store the credentials in your secrets backend (example uses `greentic-secrets` and the `dev` environment):
   ```bash
   cat <<'JSON' | greentic-secrets put secret://dev/messaging/whatsapp/acme/credentials.json
   {
     "phone_id": "1234567890",
     "wa_user_token": "EAA...",
     "app_secret": "meta-app-secret",
     "verify_token": "verify-token"
   }
   JSON
   ```
3. Export the runtime configuration for local egress (credentials are read from the secret; templates are optional overrides):
   ```bash
   export WA_TEMPLATE_NAME=weather_update
   export WA_TEMPLATE_LANG=en
   export TENANT=acme
   export NATS_URL=nats://127.0.0.1:4222
   ```
4. Run ingress and egress (pass `--features mock-http` to the binaries if you want to stub outbound calls during testing):
   ```bash
   make run-ingress-whatsapp
   make run-egress-whatsapp
   ```
5. Configure Meta to call `/ingress/whatsapp/{tenant}` with your verify token. Inbound messages publish to NATS and are delivered via egress; card responses automatically fall back to templates or deep links when the 24-hour session window has expired.

## Webex Integration

1. Create a [Webex bot](https://developer.webex.com/my-apps/new/bot) and note the Bot Access Token. Configure a webhook pointing to `/webex/messages` with a secret.
2. Export runtime configuration:
   ```bash
   export WEBEX_WEBHOOK_SECRET=super-secret
   export WEBEX_BOT_TOKEN=BearerTokenFromStep1
   export TENANT=acme
   export NATS_URL=nats://127.0.0.1:4222
   ```
3. Start the ingress and egress services:
   ```bash
   make run-ingress-webex
   make run-egress-webex
   FLOW=examples/flows/weather_webex.yaml PLATFORM=webex make run-runner
   ```
4. Set the webhook target URL (`https://<public>/webex/messages`). Messages sent to the bot are normalised, deduplicated, and republished through NATS; the egress worker handles rate limits, retries, and Adaptive Card rendering for Webex spaces.
