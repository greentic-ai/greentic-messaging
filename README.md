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

## Slack Integration

1. Create a Slack app with a Bot User token (Scopes: `app_mentions:read`, `channels:history`, `groups:history`, `im:history`, `mpim:history`, `chat:write`, `commands`). Enable Event Subscriptions and point it to `/slack/events`.
2. Export your secrets:
   ```bash
   export SLACK_SIGNING_SECRET=...
   export SLACK_BOT_TOKEN=xoxb-...
   export TENANT=acme
   export NATS_URL=nats://127.0.0.1:4222
   ```
3. Start the services:
   ```bash
   make stack-up             # optional: start local nats/docker stack
   make run-ingress-slack    # verifies URL challenge automatically
   make run-egress-slack
   FLOW=examples/flows/weather_slack.yaml PLATFORM=slack make run-runner
   ```
4. Send a message to your Slack bot (or mention it in a channel); the runner emits a `MessageCard` and Slack egress renders it as Blocks. Replies with a `thread_ts` keep the conversation threaded.

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

## Telegram Integration

1. Create a Telegram bot via BotFather and obtain the bot token; configure the webhook secret if desired.
2. Export environment variables:
   ```bash
   export TELEGRAM_SECRET_TOKEN=dev
   export TENANT=acme
   export NATS_URL=nats://127.0.0.1:4222
   ```
3. Start ingress, egress, and the runner:
```bash
make run-ingress-telegram
make run-egress-telegram
FLOW=examples/flows/weather_telegram.yaml PLATFORM=telegram make run-runner
```
4. Set the Telegram webhook to point at `/telegram/webhook`. Messages sent to the bot are normalized, routed through NATS, and responses are delivered via the Telegram egress adapter using the official Bot API.

## WebChat Integration

1. WebChat ingress/egress uses a minimal HTTP JSON interface; no external configuration is required beyond a public endpoint.
2. Export the usual runtime variables:
   ```bash
   export TENANT=acme
   export NATS_URL=nats://127.0.0.1:4222
   ```
3. Launch the services:
   ```bash
   make run-ingress-webchat
   make run-egress-webchat
   FLOW=examples/flows/weather_slack.yaml PLATFORM=webchat make run-runner
   ```
4. POST inbound messages to `/webhook` (JSON: `chat_id`, `user_id`, `text`) and open `http://localhost:8090` to view outbound updates streamed via Server-Sent Events.

## WhatsApp Integration

1. Create a WhatsApp Business App, obtain the phone number ID, and generate a permanent user access token. Configure the webhook verify token and app secret.
2. Export the required environment variables:
   ```bash
   export WA_VERIFY_TOKEN=verify-token
   export WA_APP_SECRET=meta-app-secret
   export WA_PHONE_ID=1234567890
   export WA_USER_TOKEN=EAA...
   export WA_TEMPLATE_NAME=weather_update
   export WA_TEMPLATE_LANG=en
   export TENANT=acme
   export NATS_URL=nats://127.0.0.1:4222
   ```
3. Run ingress and egress:
   ```bash
   make run-ingress-whatsapp
   make run-egress-whatsapp
   ```
4. Configure Meta to call `/whatsapp/webhook` with your verify token. Inbound messages publish to NATS and are delivered via egress; card responses automatically fall back to templates or deep links when the 24-hour session window has expired.
