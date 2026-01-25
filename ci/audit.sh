#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

echo "## Workspace members"
CARGO_NET_OFFLINE=true cargo metadata --format-version 1 --no-deps

echo
echo "## Validator dependency closure"
CARGO_NET_OFFLINE=true cargo tree -p greentic-messaging-pack-validator

echo
echo "## Cross-repo references"
rg -n "greentic-messaging" ../greentic-operator
rg -n "greentic-messaging-test" ../greentic-messaging-providers
rg -n "gsm-gateway" ../greentic-operator
rg -n "gsm-egress" ../greentic-operator

echo
echo "## CI workflows referencing scripts/crates"
rg -n "greentic-messaging-test" .github/workflows
rg -n "build-validator-pack" .github/workflows
rg -n "local_check.sh" .github/workflows
