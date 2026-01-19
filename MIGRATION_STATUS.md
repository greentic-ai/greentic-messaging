# Secrets Migration Status (greentic-messaging)

- What changed: added a deterministic secrets smoke fixture (`fixtures/packs/messaging_secrets_smoke/`) with `secret_requirements`; updated README/getting-started + provider docs to point to the `greentic-secrets` init/apply workflow; messaging-tenants now shells out to `greentic-secrets`; added component manifests with `secret_requirements` for all messaging adapters; testutil can now load credentials from a `MESSAGING_SEED_FILE` SeedDoc (warns on legacy filenames) and can disable env/SECRETS_ROOT fallbacks via `MESSAGING_DISABLE_ENV` / `MESSAGING_DISABLE_SECRETS_ROOT`; documented pack state in `packs/messaging/README.md`; added seed examples `fixtures/seeds/messaging_secrets_smoke.yaml` and `fixtures/seeds/messaging_all_smoke.yaml`; documented pack generation helper; CLI/getting-started docs mention seed-driven testing; messaging flow stubs replaced with minimal runnable smoke flows; added pack smoke test to verify flows/seed coverage.
- What broke: pack YAMLs still reference missing flow files (left untouched for PR-14); fixtures/packs do not yet include real flows/components; new component manifests under `packs/messaging/components/*/component.manifest.json` carry requirements but are not yet wired into pack builds.
- Next repos/steps: align with the updated `greentic-secrets` CLI and `secret_requirements` schema from greentic-types/pack; migrate remaining tests/helpers off env/`SECRETS_ROOT` and onto greentic-secrets seeded stores (testutil reads `MESSAGING_SEED_FILE`, prefers `messaging/<platform>.credentials.json`, and still accepts legacy `<platform>-<team>-credentials.json`); regenerate any published docs sites after in-repo docs change.

Pack inventory (flow → component → secrets)

| Pack | Flows (default/custom) | Component manifest | Secret requirements (key) |
| --- | --- | --- | --- |
| slack | flows/messaging/slack/default.ygtc, flows/messaging/slack/custom.ygtc | packs/messaging/components/slack/component.manifest.json | messaging/slack.credentials.json (Json) |
| teams | flows/messaging/teams/default.ygtc, flows/messaging/teams/custom.ygtc | packs/messaging/components/teams/component.manifest.json | messaging/teams.credentials.json (Json) |
| telegram | flows/messaging/telegram/ingress_default.ygtc, ingress_custom.ygtc, egress_default.ygtc, egress_custom.ygtc | packs/messaging/components/telegram/component.manifest.json | messaging/telegram.credentials.json (Json) |
| webchat | flows/messaging/webchat/default.ygtc, custom.ygtc | packs/messaging/components/webchat/component.manifest.json | webchat/jwt_signing_key (Text), webchat/channel_token (Text, optional) |
| webex | flows/messaging/webex/default.ygtc, custom.ygtc | packs/messaging/components/webex/component.manifest.json | messaging/webex.credentials.json (Json) |
| whatsapp | flows/messaging/whatsapp/default.ygtc, custom.ygtc | packs/messaging/components/whatsapp/component.manifest.json | messaging/whatsapp.credentials.json (Json) |
| local | flows/messaging/local/default.ygtc, custom.ygtc | none (local adapter) | none |
