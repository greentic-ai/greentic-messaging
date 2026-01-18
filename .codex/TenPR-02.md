# TenPR-02 — Remove legacy documentation and CLI exposure

GOAL  
Ensure users cannot accidentally use legacy paths.
Documentation must reflect ONLY the new stack.

TASKS

1) Remove or archive old docs:
   - legacy CLI docs
   - legacy provider docs
   - legacy subject scheme docs
   - legacy config/secrets docs

2) Create a single authoritative doc:
   - `docs/README.md`
   - Describe ONLY:
     - gateway → runner → egress
     - packs
     - dev CLI
     - DLQ

3) Update CLI help:
   - Remove legacy subcommands from help output
   - If legacy commands exist behind feature flags, hide them

4) Add explicit errors:
   - If a user tries a removed legacy command, fail with:
     “Legacy messaging is disabled. Use `messaging dev up`.”

ACCEPTANCE
- No legacy concepts appear in docs
- `--help` output shows only new CLI
