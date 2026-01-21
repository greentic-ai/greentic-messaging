#!/usr/bin/env bash
set -euo pipefail

list_crates() {
  cargo metadata --format-version 1 --no-deps \
    | jq -r '.packages[] | "\(.name) \(.version) \(.manifest_path)"'
}

crate_dir_from_manifest() {
  dirname "$1"
}
