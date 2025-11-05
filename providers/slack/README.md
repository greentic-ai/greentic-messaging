# Slack Provider Quickstart

The Slack egress adapter sends rich cards into a Slack channel using the Web API.
Follow these steps to run the contract test locally.

## Prerequisites

- A Slack workspace where you can install custom apps.
- A Slack app with a Bot token that has the `chat:write` and `chat:write.public`
  scopes (create at <https://api.slack.com/apps>).

## Configure secrets

1. From the Slack app dashboard, copy the **Bot User OAuth Token** (starts with
   `xoxb-`).
2. Open the target channel in Slack → channel header → **View channel details** →
   **More** → **Copy channel ID**.
3. Export the values (or place them in `.env`):

   ```bash
   export SLACK_BOT_TOKEN=xoxb-your-token
   export SLACK_CHANNEL_ID=C1234567890
   ```

Optional extras:

- `SLACK_SIGNING_SECRET` enables ingress request verification when running the
  webhook services.

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
