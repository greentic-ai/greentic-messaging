#!/usr/bin/env bash
set -euo pipefail

# Prints "name version manifest_path" tuples for all crates in the workspace (or single crate).
list_crates() {
  cargo metadata --format-version 1 --no-deps \
    | jq -r '.packages[] | "\(.name) \(.version) \(.manifest_path)"'
}

# Given a manifest path, returns the crate's directory.
crate_dir_from_manifest() {
  local manifest="$1"
  dirname "$manifest"
}

# Detect whether the repository defines a workspace in the root Cargo.toml.
is_workspace() {
  grep -q "^\[workspace\]" Cargo.toml && echo "1" || echo "0"
}
