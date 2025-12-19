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
ARTIFACT_DIR="$ROOT/target/components"

mkdir -p "$OUT_DIR"

if ! command -v packc >/dev/null 2>&1; then
  echo "packc not found in PATH; install with 'cargo install packc' or use the workspace binary." >&2
  exit 1
fi

echo "Checking component artifacts under ${ARTIFACT_DIR}..."
missing=0
for manifest in "$PACK_DIR"/components/*/component.manifest.json; do
  name="$(basename "$(dirname "$manifest")")"
  wasm="$ARTIFACT_DIR/${name}.wasm"
  if [ ! -f "$wasm" ]; then
    echo "missing component artifact: $wasm (build or fetch before packing)"
    missing=1
  fi
done

if [ "$missing" -ne 0 ]; then
  echo "Aborting pack generation due to missing component artifacts."
  exit 1
fi

for pack in "$PACK_DIR"/*.yaml; do
  name="$(basename "${pack%.yaml}")"
  out="$OUT_DIR/${name}.gtpack"
  echo "Packing $pack -> $out"
  packc pack "$pack" --out "$out" --component-dir "$ARTIFACT_DIR"
done
