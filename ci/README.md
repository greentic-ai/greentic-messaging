# Local CI snapshot

`ci/local_check.sh` mirrors the repo's GitHub Actions so you can fail fast before opening a PR or pushing to `master`.

## Run it

```bash
ci/local_check.sh
```

Set environment toggles inline to opt into extra behavior:

- `LOCAL_CHECK_VERBOSE=1` – enable `set -x` tracing.
- `LOCAL_CHECK_STRICT=1` – add `--locked/--all-features`, run coverage via `cargo tarpaulin`, and require optional tooling.
- `LOCAL_CHECK_ONLINE=1` – allow networked steps such as `npm ci`, downloads, and registry lookups.
- `LOCAL_CHECK_E2E=1` – execute the Playwright-powered conformance matrix (requires the secrets listed in `.github/workflows/conformance.yml`).

By default the script runs offline-safe Rust fmt/clippy/build/test steps and cleanly skips anything that would need missing tools or credentials.
