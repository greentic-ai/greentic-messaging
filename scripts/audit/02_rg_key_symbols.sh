#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
evidence_dir="$repo_root/docs/audit/_evidence/rg"
mkdir -p "$evidence_dir"

run_rg() {
  local label="$1"
  local pattern="$2"
  rg -n "$pattern" "$repo_root" > "$evidence_dir/$label.txt" || true
}

run_rg "tenant_context" "TenantCtx|TenantId|EnvId|team|user|subject|correlation"
run_rg "envelopes" "EventEnvelope|Envelope|CloudEvent|topic|source|type|subject"
run_rg "providers" "Provider|Sink|Source|Router|Webhook|Subscription"
run_rg "config" "config|Config|greentic-config|dotenv|env::var"
run_rg "secrets" "secret|Secrets|greentic-secrets"
run_rg "state_store" "state|StateStore|sqlite|rocks|redis|cache"
run_rg "overlaps_wip" "shim|compat|legacy|TODO|WIP|deprecated|feature"
