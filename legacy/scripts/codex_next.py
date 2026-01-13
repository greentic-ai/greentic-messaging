#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import os
import re
import sys
from pathlib import Path
from typing import List, Dict, Any, Optional, Tuple

ROOT = Path(__file__).resolve().parents[1]
CODEX_DIR = ROOT / ".codex"
STATE_PATH = CODEX_DIR / "STATE.json"

PR_RE = re.compile(r"^PR-(\d{2})\.md$")
FRONT_MATTER_RE = re.compile(r"^---\s*\n(.*?)\n---\s*\n", re.DOTALL)

def die(msg: str, code: int = 1) -> None:
    print(f"[codex_next] {msg}", file=sys.stderr)
    raise SystemExit(code)

def load_state() -> Dict[str, Any]:
    if not STATE_PATH.exists():
        die(f"Missing {STATE_PATH}. Create it (e.g., PR-01) before running codex_next.")
    return json.loads(STATE_PATH.read_text(encoding="utf-8"))

def save_state(state: Dict[str, Any], *, soft: bool = False) -> None:
    try:
        STATE_PATH.write_text(json.dumps(state, indent=2) + "\n", encoding="utf-8")
    except PermissionError:
        msg = (
            f"Cannot write {STATE_PATH}: permission denied. Make sure the file is writable "
            "and not marked immutable (try `chflags -R nouchg,noschg .codex`)."
        )
        if soft:
            print(f"[codex_next] {msg}", file=sys.stderr)
        else:
            die(msg)

def list_pr_files() -> List[Path]:
    if not CODEX_DIR.exists():
        die(f"Missing {CODEX_DIR}. Create .codex directory and PR files.")
    prs = []
    for p in sorted(CODEX_DIR.iterdir()):
        if p.is_file() and PR_RE.match(p.name):
            prs.append(p)
    if not prs:
        die("No PR-*.md files found in .codex/")
    return prs

def parse_front_matter(md: str) -> Dict[str, Any]:
    """
    Minimal front-matter parser (YAML-ish but very constrained).
    Supports:
      id: PR-03
      track: host|providers
      depends_on: [PR-01, PR-02]
    """
    m = FRONT_MATTER_RE.match(md)
    if not m:
        return {}
    block = m.group(1).strip().splitlines()
    out: Dict[str, Any] = {}
    for line in block:
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        if ":" not in line:
            continue
        k, v = line.split(":", 1)
        k = k.strip()
        v = v.strip()
        # list form: [A, B]
        if v.startswith("[") and v.endswith("]"):
            inner = v[1:-1].strip()
            items = [x.strip() for x in inner.split(",") if x.strip()]
            out[k] = items
        else:
            # strip simple quotes
            if (v.startswith('"') and v.endswith('"')) or (v.startswith("'") and v.endswith("'")):
                v = v[1:-1]
            out[k] = v
    return out

def repo_guard(expected_repo_hint: Optional[str]) -> None:
    """
    Optional repo guard: if STATE.json has {"repo_hint":"greentic-messaging"} etc,
    ensure we're running in the right repo root.
    """
    if not expected_repo_hint:
        return
    # cheap heuristic: root folder name match OR a Cargo.toml package name mention
    root_name = ROOT.name
    if expected_repo_hint in root_name:
        return
    cargo = ROOT / "Cargo.toml"
    if cargo.exists():
        txt = cargo.read_text(encoding="utf-8", errors="ignore")
        if expected_repo_hint in txt:
            return
    die(f"This codex_next appears to be running in '{ROOT}', but STATE.json expects repo '{expected_repo_hint}'.")

def pr_meta(path: Path) -> Tuple[str, Dict[str, Any], str]:
    md = path.read_text(encoding="utf-8")
    meta = parse_front_matter(md)
    pr_id = meta.get("id") or path.stem  # PR-03
    return pr_id, meta, md

def next_pr(state: Dict[str, Any], track: str) -> Optional[Path]:
    done = set(state.get("done", []))
    for p in list_pr_files():
        pr_id, meta, _md = pr_meta(p)
        pr_track = (meta.get("track") or state.get("default_track") or "host").strip()
        if track != "all" and pr_track != track:
            continue
        if pr_id in done:
            continue
        # dependency check
        deps = meta.get("depends_on") or []
        missing = [d for d in deps if d not in done]
        if missing:
            continue
        return p
    return None

def explain_blocked(state: Dict[str, Any], track: str) -> None:
    done = set(state.get("done", []))
    blocked: List[str] = []
    for p in list_pr_files():
        pr_id, meta, _ = pr_meta(p)
        pr_track = (meta.get("track") or state.get("default_track") or "host").strip()
        if track != "all" and pr_track != track:
            continue
        if pr_id in done:
            continue
        deps = meta.get("depends_on") or []
        missing = [d for d in deps if d not in done]
        if missing:
            blocked.append(f"{pr_id} (missing: {missing})")
    if blocked:
        print("Blocked PRs due to unmet dependencies:")
        for b in blocked:
            print(f" - {b}")

def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--status", action="store_true", help="Show current status")
    ap.add_argument("--done", type=str, help="Mark PR done, e.g. PR-02")
    ap.add_argument("--show", type=str, help="Show specific PR, e.g. PR-03")
    ap.add_argument("--track", type=str, default="all", choices=["host", "providers", "all"],
                    help="Which track to run (host/providers/all)")
    args = ap.parse_args()

    state = load_state()
    repo_guard(state.get("repo_hint"))

    if args.status:
        prs = []
        for p in list_pr_files():
            pr_id, meta, _ = pr_meta(p)
            prs.append((pr_id, meta.get("track")))
        done = set(state.get("done", []))
        pending = [pid for (pid, tr) in prs if pid not in done and (args.track == "all" or tr == args.track or tr is None)]
        print("== Codex PR Status ==")
        print(f"Repo:    {state.get('repo_hint') or ROOT.name}")
        print(f"Track:   {args.track}")
        print(f"Done:    {sorted(done)}")
        print(f"Pending: {pending}")
        print(f"Current: {state.get('current')}")
        explain_blocked(state, args.track)
        return

    if args.done:
        pr = args.done.strip()
        if pr not in state.get("done", []):
            state.setdefault("done", []).append(pr)
        state["current"] = None
        save_state(state)
        print(f"[codex_next] Marked done: {pr}")
        return

    if args.show:
        pr = args.show.strip()
        pr_path = CODEX_DIR / f"{pr}.md"
        if not pr_path.exists():
            die(f"Not found: {pr_path}")
        state["current"] = pr
        save_state(state, soft=True)
        print(pr_path.read_text(encoding="utf-8"))
        return

    p = next_pr(state, args.track)
    if p is None:
        print("[codex_next] No runnable PR found (either all done or dependencies not met).")
        explain_blocked(state, args.track)
        return

    pr_id, _meta, md = pr_meta(p)
    state["current"] = pr_id
    save_state(state, soft=True)
    print(md)

if __name__ == "__main__":
    main()
