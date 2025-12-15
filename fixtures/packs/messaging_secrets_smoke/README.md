# Messaging secrets smoke fixture

This fixture is intentionally small and deterministic so docs and tests can exercise the new `greentic-secrets` workflow without relying on real adapter binaries or external flows.

- `pack.yaml` declares a single messaging adapter entry (`secrets-smoke`) and uses `packVersion: 1`.
- `component.manifest.json` declares one `secret_requirements` entry: the adapter needs a text token at `messaging/test_api_token` with a sample scope `{env: dev, tenant: example, team: default}`.

Use it with the `greentic-secrets` CLI:

```bash
# Scaffold a seed template from the fixture
greentic-secrets scaffold --pack fixtures/packs/messaging_secrets_smoke/pack.yaml --out /tmp/seed.yaml --env dev --tenant example --team default
# Fill the token value, then apply:
greentic-secrets apply -f /tmp/seed.yaml
```

This pack is only for migration/testing; it is not shipped as part of any runtime deployment.
