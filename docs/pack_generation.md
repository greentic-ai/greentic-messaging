# Pack generation notes (messaging)

The messaging pack sources now declare `secret_requirements` alongside adapter entries. Component manifests with matching requirements live under `packs/messaging/components/**/component.manifest.json`. Flow paths exist as placeholders under `flows/messaging/**` to satisfy schema references until real flows land.

To regenerate `.gtpack` artifacts (once component binaries and real flows are available), use the helper script:

```bash
tools/generate_packs.sh
```

Expectations:
- `packc` must be available in `PATH` (`cargo install packc`).
- Component artifacts for each adapter must be discoverable by packc.
- Flow files must be valid (replace placeholders when real flows are ready).

Outputs are written to `target/packs/*.gtpack`. Wire this into CI once the component/tooling story is settled.***
