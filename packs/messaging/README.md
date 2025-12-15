# Messaging packs

This directory contains the YAML pack sources for the default messaging adapters (slack, teams, telegram, webchat, webex, whatsapp, local). Flows referenced under `flows/messaging/...` are placeholders and currently not present in the repo; they are left intact for schema compatibility and will be regenerated once the flows land.

Component manifests with `secret_requirements` live under `packs/messaging/components/*/component.manifest.json` and declare the credentials each adapter needs (e.g., `messaging/<platform>.credentials.json` or `webchat/jwt_signing_key`). Pack YAMLs now also embed `secret_requirements` mirrors to keep the sources self-describing; use either source when wiring pack generation so that `.gtpack` metadata includes the structured requirements.

For quick testing, the repository also ships a deterministic secrets smoke fixture at `fixtures/packs/messaging_secrets_smoke/` that exercises a single secret requirement without relying on the missing flows.

Pack generation:

- Placeholder flows live under `flows/messaging/**`; replace them with real flows when available.
- Use `tools/generate_packs.sh` to regenerate `.gtpack` artifacts into `target/packs/` once component artifacts and flows are ready (requires `packc` in PATH). See `docs/pack_generation.md` for details.
