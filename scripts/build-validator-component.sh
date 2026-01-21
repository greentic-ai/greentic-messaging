#!/usr/bin/env bash
set -euo pipefail

# Build the messaging pack validator wasm component and emit a digests manifest.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="${ROOT_DIR}/target/validators"
rm -rf "${OUT_DIR}"
mkdir -p "${OUT_DIR}"

VERSION="$(python3 - <<'PY'
import tomllib
from pathlib import Path
data = tomllib.loads(Path("Cargo.toml").read_text())
version = data.get("workspace", {}).get("package", {}).get("version")
if not version:
    raise SystemExit("workspace.package.version not found")
print(version)
PY
)"

crate="greentic-messaging-pack-validator"
artifact="greentic_messaging_pack_validator.wasm"

echo "Building ${crate} for version ${VERSION}"
rustup target add wasm32-wasip2 >/dev/null
cargo build -p "${crate}" --release --target wasm32-wasip2

wasm_path="${ROOT_DIR}/target/wasm32-wasip2/release/${artifact}"
if [[ ! -f "${wasm_path}" ]]; then
  echo "[ERROR] wasm artifact missing at ${wasm_path}" >&2
  exit 1
fi

cp "${wasm_path}" "${OUT_DIR}/messaging-pack-validator.wasm"

digest="$(sha256sum "${wasm_path}" | awk '{print $1}')"
ref="ghcr.io/greentic-ai/validators/messaging:${VERSION}"
digests_json="${OUT_DIR}/digests.json"
echo "[]" > "${digests_json}"

tmp="$(mktemp)"
jq --arg id "greentic.validators.messaging" \
   --arg version "${VERSION}" \
   --arg ref "${ref}" \
   --arg digest "${digest}" \
   --arg path "messaging-pack-validator.wasm" \
   '. += [{"id":$id,"version":$version,"ref":$ref,"digest":$digest,"path":$path}]' \
   "${digests_json}" > "${tmp}"
mv "${tmp}" "${digests_json}"

echo "::notice::Validator digest written to ${digests_json}"
