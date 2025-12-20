# Codex migration runner

This repo carries a lightweight migration queue to sequence small PRs. The queue is defined by the `.codex/PR-*.md` files and tracked in `.codex/STATE.json`.

## Usage

```bash
python3 scripts/codex_next.py --status         # show queue state
python3 scripts/codex_next.py                 # print the next runnable PR markdown
python3 scripts/codex_next.py --show PR-03    # print a specific PR and mark it current
python3 scripts/codex_next.py --done PR-03    # mark a PR as completed
python3 scripts/codex_next.py --track host    # limit to a track (host|providers|all)
```

`STATE.json` fields:

- `repo_hint`: optional guard to ensure the script is run from the right repo.
- `default_track`: default track when PR front-matter omits `track`.
- `current`: the PR currently being worked on.
- `done`: list of completed PR ids.

## Notes

- Dependencies are declared in each PR’s front-matter (`depends_on: [PR-01, ...]`). The runner skips PRs with unmet dependencies and reports which are blocked.
- If the runner needs to update `.codex/STATE.json`, ensure the file is writable in your environment. If you see a permission error, clear any immutable flags or adjust your sandbox to allow writes within the repo.
