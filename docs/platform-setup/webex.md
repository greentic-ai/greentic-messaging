# Webex Platform Setup

This guide walks you through creating a Webex bot, wiring a webhook to the Greentic ingress service, and providing the credentials required by the Webex egress worker. Following the steps below should get you to a working webhook URL in a few minutes.

## Prerequisites

- Webex developer account with access to <https://developer.webex.com>
- Public HTTPS endpoint (ngrok, Cloudflare Tunnel, etc.) that forwards to your running `ingress-webex` binary
- Local NATS instance (or access to the cluster used by your Greentic deployment)

## 1. Create a Webex Bot

1. Sign in to the [Webex for Developers](https://developer.webex.com/my-apps/new/bot) portal.
2. Create a new **Bot**:
   - Give it a name and mention (e.g. `Greentic Weather Bot` / `weather_bot`).
   - Copy the **Bot Access Token** – this becomes `WEBEX_BOT_TOKEN` for egress.
3. (Optional) Upload an avatar and summary so that messages look friendly in Webex spaces.

## 2. Configure the Webhook

1. From the same portal or the REST API, create a webhook with:
   - **Resource**: `messages`
   - **Event**: `created`
   - **Target URL**: `https://<public-domain>/webex/messages`
2. Set a secret string when creating the webhook. The Webex ingress service verifies signatures using this value:
   - Store the secret as `WEBEX_WEBHOOK_SECRET`.
   - You can optionally change the signature header via `WEBEX_SIG_HEADER` (defaults to `X-Webex-Signature`).
3. Add a second webhook for `attachmentActions` if you want button submit events delivered as postbacks.

## 3. Seed Secrets

Greentic expects secrets to be provided through environment variables or your secret manager. Minimum configuration:

```bash
export TENANT=acme
export NATS_URL=nats://127.0.0.1:4222
export WEBEX_WEBHOOK_SECRET=super-secret
export WEBEX_BOT_TOKEN=BearerTokenFromStep1
# Optional overrides
export WEBEX_SIG_HEADER=X-Some-Header
export WEBEX_SIG_ALGO=sha256
```

If you are using the provided tooling:

- `seed_secrets.sh` now recognises `WEBEX_BOT_TOKEN` and `WEBEX_WEBHOOK_SECRET`.
- Terraform modules surface the secrets under `infra/modules/secrets/` and expose the webhook URL via `render_outputs.sh`.

## 4. Run the Services

```bash
# Terminal 1 – ingress
TENANT=acme WEBEX_WEBHOOK_SECRET=super-secret WEBEX_SIG_ALGO=sha1 \
  WEBEX_SIG_HEADER=X-Webex-Signature make run-ingress-webex

# Terminal 2 – egress
TENANT=acme WEBEX_BOT_TOKEN=BearerTokenFromStep1 make run-egress-webex

# Terminal 3 – runner (see example flow below)
FLOW=examples/flows/weather_webex.yaml PLATFORM=webex make run-runner
```

When `ENABLE_OTEL=true` and `OTEL_EXPORTER_OTLP_ENDPOINT` is set, both services emit telemetry compatible with the dashboards under `deploy/otel/dashboards/`.

## 5. Common Errors & Fixes

| Symptom | Likely Cause | Fix |
| --- | --- | --- |
| `401 unauthorized` from ingress | Signature mismatch | Ensure `WEBEX_WEBHOOK_SECRET` matches the secret configured on the webhook and that the header name aligns with `WEBEX_SIG_HEADER`. |
| Webhook returns `5xx` | Ingress crashed while parsing payload | Run with `RUST_LOG=debug` to inspect malformed payloads. The normaliser expects Webex’s REST JSON structure. |
| Messages not delivering | Egress reports `E_CLIENT` or `E_RATE` | The bot token may be invalid or you are hitting rate limits. Check the DLQ dashboard (`dlq-heatmap.json`) and the egress logs. |
| Buttons do nothing | Missing `attachmentActions` webhook | Create the additional webhook and restart ingress so postbacks arrive in the context map. |

## 6. Next Steps

- Import the `Messages Golden Path` Grafana dashboard to observe end-to-end throughput.
- Extend the example flow in `/examples/flows/weather_webex.yaml` to invoke your own WASM tools or knowledge bases.
- Enable the unified `/a` callbacks (set `ACTION_BASE_URL` & JWT keys) to allow signed links in Webex Adaptive Cards.
