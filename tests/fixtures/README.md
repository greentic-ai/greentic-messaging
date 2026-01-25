# Fixture packs

`tests/fixtures/packs/dev-viewer-provider.gtpack` is a deterministic messaging pack used by the dev-viewer pack loader tests. It contains a single provider extension (ID `dev-viewer-provider`) with an ingress runtime. If the fixture ever needs to be rebuilt, run:

```
cargo run --manifest-path scripts/pack-fixture-builder/Cargo.toml
```

The builder script rewrites `manifest.cbor` using `greentic_types` and zips it into the expected `.gtpack`.
