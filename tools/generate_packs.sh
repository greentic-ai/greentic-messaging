#!/usr/bin/env bash
set -euo pipefail

# Helper script to (re)generate messaging .gtpack artifacts once component binaries/flows exist.
# This is a placeholder wiring to keep the secrets migration self-contained.
# Requirements:
# - packc CLI available (`cargo install packc` or use the workspace dependency)
# - component artifacts accessible to packc for each adapter referenced in packs/messaging/*.yaml
# - flow files present (currently placeholders under flows/messaging/**)

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PACK_DIR="$ROOT/packs/messaging"
OUT_DIR="$ROOT/target/packs"

mkdir -p "$OUT_DIR"

if ! command -v packc >/dev/null 2>&1; then
  echo "packc not found in PATH; install with 'cargo install packc' or use the workspace binary." >&2
  exit 1
fi

for pack in "$PACK_DIR"/*.yaml; do
  name="$(basename "${pack%.yaml}")"
  out="$OUT_DIR/${name}.gtpack"
  echo "Packing $pack -> $out"
  # This invocation assumes packc can resolve component manifests and flows referenced in the pack.
  # Adjust flags as needed when integrating with real component artifacts.
  packc pack "$pack" --out "$out"
done
