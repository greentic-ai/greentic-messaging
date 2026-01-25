#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT_DIR"

LEGACY_CRATES=(
  gsm-runner
  gsm-gateway
  gsm-egress
  gsm-subscriptions-teams
  gsm-cli-dlq
  gsm-slack-oauth
  messaging-tenants
  gsm-ingress-common
  gsm-egress-common
  gsm-backpressure
  gsm-dlq
  gsm-idempotency
  gsm-session
  gsm-translator
  gsm-testutil
  greentic-messaging-validate
  greentic-messaging-test
  gsm-bus
  greentic-messaging-security
  dev-viewer
  mock-weather-tool
  nats-demo
)

CARGO_ARGS=(test --all-targets --locked)

for crate in "${LEGACY_CRATES[@]}"; do
  printf '\n▶ legacy crate: %s\n' "$crate"
  cargo "${CARGO_ARGS[@]}" -p "$crate"
done

if [ "${LEGACY_RUN_MESSAGING_TEST:-0}" = "1" ]; then
  printf '\n▶ greentic-messaging-test harness\n'
  cargo run -p greentic-messaging-test --locked -- all --dry-run
else
  printf '[info] Set LEGACY_RUN_MESSAGING_TEST=1 to run the greentic-messaging-test harness\n'
fi
