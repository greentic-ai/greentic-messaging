#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
evidence_dir="$repo_root/docs/audit/_evidence"
mkdir -p "$evidence_dir"

cargo metadata --format-version 1 > "$evidence_dir/cargo-metadata.json"
