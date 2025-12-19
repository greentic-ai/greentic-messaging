# Pack generation notes (messaging)

The messaging pack sources now declare `secret_requirements` alongside adapter entries. Component manifests with matching requirements live under `packs/messaging/components/**/component.manifest.json`. Flow paths exist as placeholders under `flows/messaging/**` to satisfy schema references until real flows land.

To regenerate `.gtpack` artifacts (once component binaries and real flows are available), use the helper script:

```bash
tools/generate_packs.sh
```

Expectations:
- `packc` must be available in `PATH` (`cargo install packc`).
- Component artifacts for each adapter must be discoverable by packc under `target/components/<component>.wasm` (see `packs/messaging/components/*/component.manifest.json`). `tools/generate_packs.sh` will abort if any are missing.
- Flow files must be valid (placeholders replaced with minimal runnable smoke flows).

Outputs are written to `target/packs/*.gtpack`. Wire this into CI once the component/tooling story is settled.

Smoke secrets:

- Use `fixtures/seeds/messaging_all_smoke.yaml` for seed-based runs (set `MESSAGING_SEED_FILE` and disable env fallbacks with `MESSAGING_DISABLE_ENV=1` and `MESSAGING_DISABLE_SECRETS_ROOT=1`).
- For a single-adapter demo, `fixtures/packs/messaging_secrets_smoke/pack.yaml` + `fixtures/seeds/messaging_secrets_smoke.yaml` also work.
