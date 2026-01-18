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
# Adapter packs are shipped as signed .gtpack archives (or raw YAML during development). Point `MESSAGING_ADAPTER_PACK_PATHS` at the `.gtpack` file directly; relative paths are resolved under `MESSAGING_PACKS_ROOT` (default `packs/`).
```

## Seed messaging secrets (dev)

Use the canonical secrets CLI to scaffold and apply requirements for the messaging pack. The repo ships a minimal deterministic fixture you can start with:

```bash
# Create dev context + scaffold/apply using the smoke fixture
greentic-secrets init --pack fixtures/packs/messaging_secrets_smoke/pack.yaml \
  --env dev --tenant acme --team default --non-interactive
```

Replace `env`/`tenant`/`team` and the seed values as needed for your setup. Avoid the legacy `./secrets` folder and ad hoc env vars; `greentic-secrets` is the canonical workflow.

### Testing with seed files

When running tests or local helpers, you can point at a seed file instead of env/`SECRETS_ROOT`:

```bash
export MESSAGING_SEED_FILE=fixtures/seeds/messaging_secrets_smoke.yaml
export MESSAGING_DISABLE_ENV=1
export MESSAGING_DISABLE_SECRETS_ROOT=1
```

This forces test utilities to use the seed and ignore legacy env/dir fallbacks.

## Use gtpack adapters with greentic-messaging

1) Build or fetch `.gtpack` bundles  
   - Dev: `tools/generate_packs.sh` expects component WASM artifacts under `target/components/*.wasm` and emits `target/packs/*.gtpack`.  
   - CI/consumers: use the signed packs published alongside releases or artifacts you trust.

2) Start gateway/egress with the packs loaded  
   - Via CLI: `greentic-messaging serve ingress slack --tenant acme --pack target/packs/greentic-messaging-slack.gtpack` (repeat `--pack` for multiple bundles).  
   - Env-based: set `MESSAGING_PACKS_ROOT` and `MESSAGING_ADAPTER_PACK_PATHS=/abs/path/a.gtpack,/abs/path/b.gtpack`. Default packs can also be enabled with `MESSAGING_INSTALL_ALL_DEFAULT_ADAPTER_PACKS=true`.

3) Invoke components via greentic-runner  
   - Set `MESSAGING_RUNNER_HTTP_URL=https://runner-host/invoke` (and optionally `MESSAGING_RUNNER_HTTP_API_KEY`) so messaging-egress calls greentic-runner’s HTTP endpoint for each outbound envelope, passing the pack’s component id and flow path.  
   - If unset, a logging runner client is used. Bus publish still occurs for legacy consumers. Run `make run-runner FLOW=flows/messaging/slack/default.ygtc PLATFORM=slack` locally to supply the runner endpoint during development.

## Choose Your Platform

| Platform | Setup Guide | Example Flow | Notes |
| --- | --- | --- | --- |
| Slack | README section “Slack Integration” | `examples/flows/weather_slack.yaml` | Seed Slack creds via `greentic-secrets init/apply`; tokens not exported via env anymore. |
| Microsoft Teams | README section “Microsoft Teams Integration” | — | Seed client credentials via `greentic-secrets`; Graph subscriptions still supported (no example flow shipped yet). |
| Telegram | README section “Telegram Integration” | `examples/flows/weather_telegram.yaml` | Works with BotFather bots; secrets flow via `greentic-secrets`. |
| WebChat | README section “WebChat Integration” | — | Minimal HTTP+SSE demo; secrets seeded via `greentic-secrets` (example flow not shipped). |
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
