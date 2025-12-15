# Slack Provider Quickstart

> Seed credentials via `greentic-secrets` (ctx + scaffold/wizard/apply). The env-var examples below are legacy and will be removed; prefer `greentic-secrets init --pack <pack>` with the messaging pack metadata.

The Slack egress adapter sends rich cards into a Slack channel using the Web API.
Follow these steps to run the contract test locally.

## Prerequisites

- A Slack workspace where you can install custom apps.
- A Slack app with a Bot token that has the `chat:write` and `chat:write.public`
  scopes (create at <https://api.slack.com/apps>).

## Configure secrets

Preferred (greentic-secrets):

1. From the Slack app dashboard, copy the **Bot User OAuth Token** (starts with `xoxb-`) and the channel ID.
2. Scaffold and apply a seed via `greentic-secrets`:

   ```bash
   greentic-secrets scaffold --pack fixtures/packs/messaging_secrets_smoke/pack.yaml --out /tmp/slack-seed.yaml --env dev --tenant acme --team default
   # Edit /tmp/slack-seed.yaml to include:
   # messaging/slack.credentials.json:
   #   bot_token: xoxb-...
   #   channel_id: C1234567890
   #   signing_secret: optional
   greentic-secrets apply -f /tmp/slack-seed.yaml
   ```

Legacy (deprecated) env setup:

```bash
export SLACK_BOT_TOKEN=xoxb-your-token
export SLACK_CHANNEL_ID=C1234567890
# optional
export SLACK_SIGNING_SECRET=your-signing-secret
```

## Run the contract test

```bash
make stack-up
make conformance-slack
make stack-down
```

The test posts a “Daily Weather” card to the configured channel, fetches the
message history, and asserts that the blocks render correctly. Artifacts (payload
JSON, screenshots) land in:

- `tools/playwright/output/`
- `target/e2e-artifacts/`

## Troubleshooting

- Missing scopes manifest as `chat.postMessage` errors in the test output; add
  the required scopes and reinstall the app.
- Use a dedicated test channel so automated cards do not disturb production
  conversations.
