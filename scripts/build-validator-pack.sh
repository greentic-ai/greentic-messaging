#!/usr/bin/env bash
set -euo pipefail

# Build messaging validator .gtpack bundle from ./validators/messaging using packc.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="${ROOT_DIR}/dist"
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

src="${ROOT_DIR}/validators/messaging"
if [[ ! -d "${src}" ]]; then
  echo "missing pack source: ${src}" >&2
  exit 1
fi

if ! bash "${ROOT_DIR}/scripts/build-validator-component.sh"; then
  echo "validator component build failed" >&2
  exit 1
fi

staging="${OUT_DIR}/validators-messaging"
rm -rf "${staging}"
mkdir -p "${staging}"
rsync -a "${src}/" "${staging}/"

component_src="${ROOT_DIR}/target/validators/messaging-pack-validator.wasm"
component_dst="${staging}/components/greentic.validators.messaging.wasm"
mkdir -p "$(dirname "${component_dst}")"
cp "${component_src}" "${component_dst}"

if [[ -f "${staging}/pack.yaml" ]] && grep -q '__PACK_VERSION__' "${staging}/pack.yaml"; then
  sed -i.bak "s/__PACK_VERSION__/${VERSION}/g" "${staging}/pack.yaml"
  rm -f "${staging}/pack.yaml.bak"
fi

greentic-pack build \
  --in "${staging}" \
  --gtpack-out "${OUT_DIR}/validators-messaging.gtpack" \
  --no-update

greentic-pack doctor \
  --pack "${OUT_DIR}/validators-messaging.gtpack" \
  --offline \
  --allow-oci-tags

echo "::notice::built pack validators-messaging.gtpack"
