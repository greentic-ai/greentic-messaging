# Getting Started

The Greentic runtime ships several ready-to-run examples showing how to ingest messages, call tools, and send formatted responses across multiple chat platforms. This quick-start highlights the common prerequisites and points you to platform-specific setup guides.

## Prerequisites

- Rust toolchain (`rustup` + stable toolchain)
- NATS server (local `docker compose up nats` or remote cluster URL)
- `make` (used by helper targets under the root `Makefile`)
- Optional: OTLP collector if you want telemetry dashboards (see `deploy/otel/`)

## Common Environment Variables

```bash
export TENANT=acme
export NATS_URL=nats://127.0.0.1:4222
# Enable telemetry when a collector is available
# export ENABLE_OTEL=true
# export OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317
```

## Choose Your Platform

| Platform | Setup Guide | Example Flow | Notes |
| --- | --- | --- | --- |
| Slack | README section “Slack Integration” | `examples/flows/weather_slack.yaml` | Requires Slack Bot token + signing secret |
| Microsoft Teams | README section “Microsoft Teams Integration” | `examples/flows/weather_telegram.yaml` | Uses Graph subscriptions + adaptive cards |
| Telegram | README section “Telegram Integration” | `examples/flows/weather_telegram.yaml` | Works with BotFather bots |
| WebChat | README section “WebChat Integration” | `examples/flows/weather_slack.yaml` | Minimal HTTP+SSE demo |
| WhatsApp | README section “WhatsApp Integration” | — | Requires Meta Business setup |
| **Webex** | [`docs/platform-setup/webex.md`](platform-setup/webex.md) | `examples/flows/weather_webex.yaml` | New Webex ingress/egress pipeline |

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
