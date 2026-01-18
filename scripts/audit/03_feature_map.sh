#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
evidence_dir="$repo_root/docs/audit/_evidence/features"
metadata_path="$repo_root/docs/audit/_evidence/cargo-metadata.json"
mkdir -p "$evidence_dir"

if [[ ! -f "$metadata_path" ]]; then
  cargo metadata --format-version 1 > "$metadata_path"
fi

export REPO_ROOT="$repo_root"
python3 - <<'PY'
import json
import os

repo_root = os.environ['REPO_ROOT']
features_dir = os.path.join(repo_root, 'docs', 'audit', '_evidence', 'features')
metadata_path = os.path.join(repo_root, 'docs', 'audit', '_evidence', 'cargo-metadata.json')

with open(metadata_path, 'r', encoding='utf-8') as f:
    meta = json.load(f)

rows = []
for pkg in sorted(meta.get('packages', []), key=lambda p: p.get('name')):
    name = pkg.get('name')
    feats = pkg.get('features', {})
    feat_names = sorted(feats.keys())
    rows.append((name, feat_names))

out_path = os.path.join(features_dir, 'by-crate.md')
with open(out_path, 'w', encoding='utf-8') as f:
    f.write('| crate | features |\n')
    f.write('|---|---|\n')
    for name, feat_names in rows:
        feat_str = ', '.join(feat_names) if feat_names else '(none)'
        f.write(f'| {name} | {feat_str} |\n')
PY

rg -n "cfg\(feature" "$repo_root" > "$evidence_dir/flags-mentioned-in-code.txt" || true
