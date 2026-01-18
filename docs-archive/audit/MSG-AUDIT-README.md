# MSG-AUDIT README

This audit documents how greentic-messaging is wired today (binaries, data flows, tenants, envelopes, providers, config/secrets/state, storage/caches) and where overlaps/shims/WIP exist. It is descriptive only and cites evidence from code and generated outputs.

## Index

- [01-crate-map.md](01-crate-map.md)
- [02-runtime-flows.md](02-runtime-flows.md)
- [03-tenancy-and-context.md](03-tenancy-and-context.md)
- [04-event-envelope-and-topics.md](04-event-envelope-and-topics.md)
- [05-providers-sources-sinks.md](05-providers-sources-sinks.md)
- [06-config-secrets-state.md](06-config-secrets-state.md)
- [07-storage-and-caches.md](07-storage-and-caches.md)
- [08-overlaps-shims-wip.md](08-overlaps-shims-wip.md)

## Evidence Artifacts

Generated evidence lives under `docs/audit/_evidence/`:

- `docs/audit/_evidence/cargo-metadata.json`
- `docs/audit/_evidence/cargo-tree/*.txt`
- `docs/audit/_evidence/rg/*.txt`
- `docs/audit/_evidence/features/by-crate.md`
- `docs/audit/_evidence/features/flags-mentioned-in-code.txt`

Scripts to regenerate evidence (read-only):

- `scripts/audit/00_dump_metadata.sh`
- `scripts/audit/01_dump_trees.sh`
- `scripts/audit/02_rg_key_symbols.sh`
- `scripts/audit/03_feature_map.sh`

## Notes

- External crates (e.g., `greentic-types`, `greentic-session`) are referenced via usage sites in this repo; their internal definitions are treated as Known Unknowns where needed.
