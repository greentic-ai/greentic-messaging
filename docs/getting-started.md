# Getting Started

The Greentic runtime ships several ready-to-run examples showing how to ingest messages, call tools, and send formatted responses across multiple chat platforms. This quick-start highlights the common prerequisites and points you to platform-specific setup guides.

## Prerequisites

- Rust toolchain (`rustup` + stable toolchain)
- NATS server (local `docker compose up nats` or remote cluster URL)
- `make` (used by helper targets under the root `Makefile`)
- Optional: OTLP collector if you want telemetry dashboards (see `deploy/otel/`)
- Secrets seeded via the `greentic-secrets` CLI (ctx + seed/apply)

## Common Environment Variables

```bash
export NATS_URL=nats://127.0.0.1:4222
export OAUTH_BASE_URL=https://oauth.greentic.dev
# Enable telemetry when a collector is available
# export ENABLE_OTEL=true
# export OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317
```

## Seed messaging secrets (dev)

Use the canonical secrets CLI to scaffold and apply requirements for the messaging pack. The repo ships a minimal deterministic fixture you can start with:

```bash
# Create dev context + scaffold/apply using the smoke fixture
greentic-secrets init --pack fixtures/packs/messaging_secrets_smoke/pack.yaml \
  --env dev --tenant acme --team default --non-interactive
```

Replace `env`/`tenant`/`team` and the seed values as needed for your setup. Avoid the legacy `./secrets` folder and ad hoc env vars; `greentic-secrets` is the canonical workflow.

## Choose Your Platform

| Platform | Setup Guide | Example Flow | Notes |
| --- | --- | --- | --- |
| Slack | README section “Slack Integration” | `examples/flows/weather_slack.yaml` | Seed Slack creds via `greentic-secrets init/apply`; tokens not exported via env anymore. |
| Microsoft Teams | README section “Microsoft Teams Integration” | `examples/flows/weather_telegram.yaml` | Seed client credentials via `greentic-secrets`; Graph subscriptions still supported. |
| Telegram | README section “Telegram Integration” | `examples/flows/weather_telegram.yaml` | Works with BotFather bots; secrets flow via `greentic-secrets`. |
| WebChat | README section “WebChat Integration” | `examples/flows/weather_slack.yaml` | Minimal HTTP+SSE demo; secrets seeded via `greentic-secrets`. |
| WhatsApp | README section “WhatsApp Integration” | — | Requires Meta Business setup; seed tokens via `greentic-secrets`. |
| **Webex** | [`docs/platform-setup/webex.md`](platform-setup/webex.md) | `examples/flows/weather_webex.yaml` | New Webex ingress/egress pipeline; seed webhook/bot secrets via `greentic-secrets`. |
| **Dev Viewer** | README section “Golden Fixtures & Previewing” | `libs/core/tests/fixtures/cards/*.json` | Run `cargo run -p dev-viewer -- --listen 127.0.0.1:7878` and open the UI to preview all platforms without touching a bot |

## Running a Demo Flow

After completing the platform-specific setup, start the relevant services and the runner. Example (Webex):

```bash
# Terminal 1 – ingress
make run-ingress-webex

# Terminal 2 – egress
make run-egress-webex

# Terminal 3 – runner
FLOW=examples/flows/weather_webex.yaml PLATFORM=webex make run-runner
```

Chat with your bot on the configured platform to trigger the flow. Metrics and traces appear automatically when telemetry is enabled.
