#!/usr/bin/env bash
set -euo pipefail

# Usage:
#   LOCAL_CHECK_ONLINE=1 LOCAL_CHECK_STRICT=1 ci/local_check.sh
# Defaults: online, non-strict, quiet. Set LOCAL_CHECK_VERBOSE=1 for trace logs.

ONLINE="${LOCAL_CHECK_ONLINE:-1}"
STRICT="${LOCAL_CHECK_STRICT:-0}"
VERBOSE="${LOCAL_CHECK_VERBOSE:-0}"
E2E="${LOCAL_CHECK_E2E:-0}"
SKIP_CODE=97
TOTAL_STEPS=0
PASSED_STEPS=0
SKIPPED_STEPS=0
STACK_STARTED=0
CARGO_OFFLINE_READY=""
declare -a CARGO_OFFLINE_ARGS=()

export CARGO_TERM_COLOR=always
if [ "$ONLINE" != "1" ]; then
  export CARGO_NET_OFFLINE=true
  CARGO_OFFLINE_ARGS=(--offline)
else
  CARGO_OFFLINE_ARGS=()
fi

if [ "$VERBOSE" = "1" ]; then
  set -x
fi

have() {
  command -v "$1" >/dev/null 2>&1
}

need() {
  if have "$1"; then
    return 0
  fi
  printf '[miss] %s\n' "$1"
  return 1
}

step() {
  printf '\n▶ %s\n' "$*"
}

skip() {
  return "$SKIP_CODE"
}

run_or_skip() {
  local desc="$1"
  shift
  TOTAL_STEPS=$((TOTAL_STEPS + 1))
  set +e
  "$@"
  local status=$?
  set -e
  if [ "$status" -eq 0 ]; then
    PASSED_STEPS=$((PASSED_STEPS + 1))
    return 0
  fi
  if [ "$status" -eq "$SKIP_CODE" ]; then
    SKIPPED_STEPS=$((SKIPPED_STEPS + 1))
    printf '[skip] %s\n' "$desc"
    return 0
  fi
  printf '[fail] %s (exit %s)\n' "$desc" "$status"
  exit "$status"
}

want_online() {
  local desc="$1"
  if [ "$ONLINE" = "1" ]; then
    return 0
  fi
  printf '[offline] %s (set LOCAL_CHECK_ONLINE=1 to run)\n' "$desc"
  skip
}

ensure_tool() {
  local tool="$1"
  if need "$tool"; then
    return 0
  fi
  if [ "$STRICT" = "1" ]; then
    printf '[fail] Missing required tool: %s (strict mode)\n' "$tool"
    return 1
  fi
  skip
}

ensure_tools() {
  local tool
  for tool in "$@"; do
    ensure_tool "$tool"
    local status=$?
    if [ "$status" -ne 0 ]; then
      return "$status"
    fi
  done
  return 0
}

cleanup_stack() {
  if [ "$STACK_STARTED" -eq 1 ]; then
    printf '\n[cleanup] Tearing down docker stack\n'
    if have make; then
      make stack-down >/dev/null 2>&1 || true
    fi
    STACK_STARTED=0
  fi
}

trap cleanup_stack EXIT

print_tool_versions() {
  local printed=0
  local tool
  for tool in rustc cargo npm node npx docker make; do
    if have "$tool"; then
      "$tool" --version
      printed=1
    fi
  done
  if [ "$printed" -eq 0 ]; then
    printf '[warn] No known tool versions could be printed\n'
  fi
}

ensure_cargo_cache() {
  if [ "$ONLINE" = "1" ]; then
    return 0
  fi
  if [ "$CARGO_OFFLINE_READY" = "1" ]; then
    return 0
  fi
  if [ "$CARGO_OFFLINE_READY" = "0" ]; then
    printf '[offline] cargo registry cache missing; skipping build/test steps\n'
    return "$SKIP_CODE"
  fi
  if cargo fetch --offline >/dev/null 2>&1; then
    CARGO_OFFLINE_READY="1"
    return 0
  fi
  CARGO_OFFLINE_READY="0"
  printf '[offline] cargo registry cache missing; skipping build/test steps\n'
  return "$SKIP_CODE"
}

rustfmt_step() {
  ensure_tool cargo
  local status=$?
  if [ "$status" -ne 0 ]; then
    return "$status"
  fi
  cargo fmt --all -- --check
}

clippy_step() {
  ensure_tool cargo
  local status=$?
  if [ "$status" -ne 0 ]; then
    return "$status"
  fi
  ensure_cargo_cache
  status=$?
  if [ "$status" -ne 0 ]; then
    return "$status"
  fi
  local args=(clippy --workspace --all-targets --locked)
  args+=("${CARGO_OFFLINE_ARGS[@]}")
  if [ "$STRICT" = "1" ]; then
    args+=(--all-features)
  fi
  local lint_args=(-Aclippy::uninlined-format-args -D warnings)
  cargo "${args[@]}" -- "${lint_args[@]}"
}

build_step() {
  ensure_tool cargo
  local status=$?
  if [ "$status" -ne 0 ]; then
    return "$status"
  fi
  ensure_cargo_cache
  status=$?
  if [ "$status" -ne 0 ]; then
    return "$status"
  fi
  local args=(build --workspace --all-targets --locked)
  args+=("${CARGO_OFFLINE_ARGS[@]}")
  if [ "$STRICT" = "1" ]; then
    args+=(--all-features)
  fi
  cargo "${args[@]}"
}

build_all_features_step() {
  ensure_tool cargo
  local status=$?
  if [ "$status" -ne 0 ]; then
    return "$status"
  fi
  ensure_cargo_cache
  status=$?
  if [ "$status" -ne 0 ]; then
    return "$status"
  fi
  cargo build --workspace --all-features --locked "${CARGO_OFFLINE_ARGS[@]}"
}

test_step() {
  ensure_tool cargo
  local status=$?
  if [ "$status" -ne 0 ]; then
    return "$status"
  fi
  ensure_cargo_cache
  status=$?
  if [ "$status" -ne 0 ]; then
    return "$status"
  fi
  local args=(test --workspace --all-targets --locked)
  args+=("${CARGO_OFFLINE_ARGS[@]}")
  if [ "$STRICT" = "1" ]; then
    args+=(--all-features)
    cargo "${args[@]}" -- --nocapture
    return $?
  fi
  cargo "${args[@]}"
}

test_all_features_step() {
  ensure_tool cargo
  local status=$?
  if [ "$status" -ne 0 ]; then
    return "$status"
  fi
  ensure_cargo_cache
  status=$?
  if [ "$status" -ne 0 ]; then
    return "$status"
  fi
  cargo test --workspace --all-features --locked "${CARGO_OFFLINE_ARGS[@]}" -- --nocapture
}

run_messaging_test_step() {
  ensure_tool cargo
  local status=$?
  if [ "$status" -ne 0 ]; then
    return "$status"
  fi
  ensure_cargo_cache
  status=$?
  if [ "$status" -ne 0 ]; then
    return "$status"
  fi
  cargo run -p greentic-messaging-test --locked "${CARGO_OFFLINE_ARGS[@]}" -- all --dry-run
}

validator_pack_step() {
  ensure_tools cargo python3 jq rsync greentic-pack
  local status=$?
  if [ "$status" -ne 0 ]; then
    return "$status"
  fi
  ensure_cargo_cache
  status=$?
  if [ "$status" -ne 0 ]; then
    return "$status"
  fi
  scripts/build-validator-pack.sh
}

coverage_step() {
  if [ "$STRICT" != "1" ]; then
    printf '[info] Coverage runs only when LOCAL_CHECK_STRICT=1\n'
    return "$SKIP_CODE"
  fi
  ensure_tool cargo
  local status=$?
  if [ "$status" -ne 0 ]; then
    return "$status"
  fi
  if ! have cargo-tarpaulin; then
    printf '[warn] cargo-tarpaulin is not installed (cargo install cargo-tarpaulin)\n'
    if [ "$STRICT" = "1" ]; then
      return 1
    fi
    return "$SKIP_CODE"
  fi
  cargo tarpaulin --workspace --all-features --locked --out Lcov --output-dir coverage
}

ensure_stack_up() {
  if [ "$STACK_STARTED" -eq 1 ]; then
    return 0
  fi
  ensure_tools make docker
  local status=$?
  if [ "$status" -ne 0 ]; then
    return "$status"
  fi
  make stack-up
  STACK_STARTED=1
}

prepare_playwright_deps() {
  local path
  for path in tools/playwright tools/renderers; do
    if [ -f "$path/package-lock.json" ]; then
      (cd "$path" && npm ci)
    fi
  done
  if [ -d tools/playwright ]; then
    (cd tools/playwright && npx playwright install)
    rm -rf tools/playwright/output || true
    mkdir -p tools/playwright/output
  fi
  rm -rf target/e2e-artifacts || true
  mkdir -p target/e2e-artifacts
}

CONFORMANCE_ENTRIES=(
  "slack|conformance-slack|SLACK_BOT_TOKEN,SLACK_CHANNEL_ID|"
  "telegram|conformance-telegram|TELEGRAM_BOT_TOKEN|TELEGRAM_CHAT_ID,TELEGRAM_CHAT_HANDLE"
  "webex|conformance-webex|WEBEX_BOT_TOKEN,WEBEX_ROOM_ID|"
  "whatsapp|conformance-whatsapp|WHATSAPP_TOKEN,WHATSAPP_PHONE_ID,WHATSAPP_RECIPIENT|"
  "teams|conformance-teams|TEAMS_TENANT_ID,TEAMS_CLIENT_ID,TEAMS_CLIENT_SECRET,TEAMS_CHAT_ID|"
  "webchat|conformance-webchat|WEBCHAT_DIRECT_LINE_SECRET,WEBCHAT_ENV,WEBCHAT_TENANT|"
)

conformance_requirements_met() {
  local platform="$1"
  local required="$2"
  local any_of="$3"
  local missing=()

  if [ -n "$required" ]; then
    IFS=',' read -r -a reqs <<<"$required"
    local key
    for key in "${reqs[@]}"; do
      [ -z "$key" ] && continue
      if [ -z "${!key:-}" ]; then
        missing+=("$key")
      fi
    done
  fi

  if [ -n "$any_of" ]; then
    IFS=',' read -r -a anys <<<"$any_of"
    local satisfied=0
    local key
    for key in "${anys[@]}"; do
      [ -z "$key" ] && continue
      if [ -n "${!key:-}" ]; then
        satisfied=1
        break
      fi
    done
    if [ "$satisfied" -eq 0 ]; then
      missing+=("one of {$any_of}")
    fi
  fi

  if [ "${#missing[@]}" -gt 0 ]; then
    printf '[info] Skipping %s conformance (missing %s)\n' "$platform" "${missing[*]}"
    return "$SKIP_CODE"
  fi
  return 0
}

run_conformance_suite() {
  if [ "$E2E" != "1" ]; then
    printf '[info] Set LOCAL_CHECK_E2E=1 to exercise Playwright conformance suites\n'
    return "$SKIP_CODE"
  fi
  want_online "Conformance matrix"
  local status=$?
  if [ "$status" -ne 0 ]; then
    return "$status"
  fi
  if [ ! -d tools/playwright ]; then
    printf '[warn] tools/playwright not found; skipping conformance suite\n'
    return "$SKIP_CODE"
  fi
  ensure_tools cargo npm npx node make docker
  status=$?
  if [ "$status" -ne 0 ]; then
    return "$status"
  fi
  prepare_playwright_deps

  local ran_any=0
  local entry
  for entry in "${CONFORMANCE_ENTRIES[@]}"; do
    IFS='|' read -r platform target required any_of <<<"$entry"
    conformance_requirements_met "$platform" "$required" "$any_of"
    status=$?
    if [ "$status" -ne 0 ]; then
      if [ "$status" -eq "$SKIP_CODE" ]; then
        continue
      fi
      return "$status"
    fi
    ensure_stack_up
    status=$?
    if [ "$status" -ne 0 ]; then
      return "$status"
    fi
    step "Conformance: $platform"
    if ! make "$target"; then
      return 1
    fi
    ran_any=1
  done

  if [ "$ran_any" -eq 0 ]; then
    printf '[info] No conformance targets enabled by environment vars\n'
    return "$SKIP_CODE"
  fi
  return 0
}

main() {
  step "Tool versions"
  run_or_skip "tool versions" print_tool_versions

  step "cargo fmt --check"
  run_or_skip "cargo fmt" rustfmt_step

  step "cargo clippy"
  run_or_skip "cargo clippy" clippy_step

  step "greentic-messaging-test"
  run_or_skip "greentic-messaging-test all --dry-run" run_messaging_test_step

  step "cargo build"
  run_or_skip "cargo build" build_step

  step "validator pack"
  run_or_skip "validator pack" validator_pack_step

  step "cargo build (all features)"
  run_or_skip "cargo build --all-features" build_all_features_step

  step "cargo test"
  run_or_skip "cargo test" test_step

  step "cargo test (all features)"
  run_or_skip "cargo test --all-features" test_all_features_step

  step "Coverage (tarpaulin)"
  run_or_skip "cargo tarpaulin" coverage_step

  step "Conformance (Playwright matrix)"
  run_or_skip "conformance suite" run_conformance_suite

  printf '\n[done] %s steps: %s passed, %s skipped.\n' "$TOTAL_STEPS" "$PASSED_STEPS" "$SKIPPED_STEPS"
}

main "$@"
