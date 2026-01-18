#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
evidence_dir="$repo_root/docs/audit/_evidence/cargo-tree"
metadata_path="$repo_root/docs/audit/_evidence/cargo-metadata.json"
mkdir -p "$evidence_dir"

if [[ ! -f "$metadata_path" ]]; then
  cargo metadata --format-version 1 > "$metadata_path"
fi

export REPO_ROOT="$repo_root"
python3 - <<'PY'
import json
import os
import subprocess

repo_root = os.environ['REPO_ROOT']
evidence_dir = os.path.join(repo_root, 'docs', 'audit', '_evidence', 'cargo-tree')
metadata_path = os.path.join(repo_root, 'docs', 'audit', '_evidence', 'cargo-metadata.json')

with open(metadata_path, 'r', encoding='utf-8') as f:
    meta = json.load(f)

bin_targets = []
workspace_members = set(meta.get('workspace_members', []))
packages_by_id = {pkg.get('id'): pkg for pkg in meta.get('packages', [])}

for pkg_id in workspace_members:
    pkg = packages_by_id.get(pkg_id)
    if not pkg:
        continue
    pkg_name = pkg.get('name')
    for tgt in pkg.get('targets', []):
        kinds = tgt.get('kind', [])
        if 'bin' in kinds:
            bin_targets.append((pkg_name, tgt.get('name')))

seen = {}
for pkg_name, bin_name in sorted(bin_targets):
    key = bin_name
    if key in seen:
        seen[key] += 1
        key = f"{bin_name}-{pkg_name}"
    else:
        seen[key] = 1
    out_path = os.path.join(evidence_dir, f"{key}.txt")
    cmd = [
        'cargo', 'tree',
        '-p', pkg_name,
        '--bin', bin_name,
    ]
    with open(out_path, 'w', encoding='utf-8') as f:
        subprocess.run(cmd, check=False, stdout=f, stderr=subprocess.STDOUT)
PY
