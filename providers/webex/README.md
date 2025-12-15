# Webex Provider Quickstart

> Seed credentials via `greentic-secrets` (ctx + scaffold/wizard/apply). The env-var examples below are legacy and will be removed; prefer `greentic-secrets init --pack <pack>` with the messaging pack metadata.

The Webex adapter posts Adaptive Cards to rooms via the Webex REST API. Follow
these steps to validate the integration locally.

## Prerequisites

- A Webex developer account (<https://developer.webex.com>).
- A bot created in the Webex portal with its **Bot Access Token**.
- A space (room) where the bot is a member.

## Configure secrets

Preferred (greentic-secrets):

```bash
greentic-secrets scaffold --pack fixtures/packs/messaging_secrets_smoke/pack.yaml --out /tmp/webex-seed.yaml --env dev --tenant acme --team default
# Edit /tmp/webex-seed.yaml to include:
# messaging/webex.credentials.json:
#   bot_token: ZjQyY...
#   room_id: Y2lzY29zcGFyazovL3VzL1JPT00v...
greentic-secrets apply -f /tmp/webex-seed.yaml
```

Legacy (deprecated) env setup:

1. Copy the bot access token from the Webex developer dashboard.
2. Open the target space in a browser and copy the Room ID from the URL (the
   portion after `/rooms/`), or fetch it via the REST API.
3. Export the values:

   ```bash
   export WEBEX_BOT_TOKEN=ZjQyY...
   export WEBEX_ROOM_ID=Y2lzY29zcGFyazovL3VzL1JPT00v...
   ```

## Run the contract test

```bash
make stack-up
make conformance-webex
make stack-down
```

The test sends an approval Adaptive Card, reads it back, and asserts the content
type and structure. Artifacts (API payloads, screenshots) land in
`tools/playwright/output/` and `target/e2e-artifacts/`.

## Troubleshooting

- A `401 Unauthorized` response typically means the bot token was revoked—issue
  a new token in the portal and re-export it.
- Ensure the bot is added to the room before running the test; otherwise Webex
  returns `404` for missing messages.
