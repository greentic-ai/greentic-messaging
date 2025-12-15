# Microsoft Teams Provider Quickstart

> Seed credentials via `greentic-secrets` (ctx + scaffold/wizard/apply). The env-var examples below are legacy and will be removed; prefer `greentic-secrets init --pack <pack>` with the messaging pack metadata.

The Teams adapter posts Adaptive Cards through Microsoft Graph. The contract test
expects a service principal with application permissions to send messages into a
chat.

## Prerequisites

- An Azure AD application (App Registration) with the **ChatMessage.Send**
  *application* permission granted and admin-consented.
- A client secret for that application.
- The target chat ID (group chat or 1:1) where the bot will post.

## Configure secrets

Preferred (greentic-secrets):

```bash
greentic-secrets scaffold --pack fixtures/packs/messaging_secrets_smoke/pack.yaml --out /tmp/teams-seed.yaml --env dev --tenant acme --team default
# Edit /tmp/teams-seed.yaml to include:
# messaging/teams.credentials.json:
#   tenant_id: 00000000-0000-0000-0000-000000000000
#   client_id: 11111111-1111-1111-1111-111111111111
#   client_secret: super-secret
greentic-secrets apply -f /tmp/teams-seed.yaml
```

Legacy (deprecated) env setup:

```bash
export TEAMS_TENANT_ID=00000000-0000-0000-0000-000000000000
export TEAMS_CLIENT_ID=11111111-1111-1111-1111-111111111111
export TEAMS_CLIENT_SECRET=super-secret
export TEAMS_CHAT_ID=19:abc123def456@thread.v2
```

Need help discovering the chat ID? Run the helper:

```bash
cargo run --manifest-path scripts/Cargo.toml --bin teams_setup \
  --tenant "$TEAMS_TENANT_ID" \
  --client-id "$TEAMS_CLIENT_ID" \
  --client-secret "$TEAMS_CLIENT_SECRET" \
  --chat-id "$TEAMS_CHAT_ID" \
  --output .env
```

It validates access and appends the values to the provided `.env`.

## Run the contract test

```bash
make stack-up
make conformance-teams
make stack-down
```

The test posts the approval Adaptive Card, fetches the Graph message detail, and
asserts that the attachment matches expectations. Artifacts are stored under
`tools/playwright/output/` and `target/e2e-artifacts/`.

## Troubleshooting

- `401` or `403` responses mean the application permission is missing or not
  admin-consented—double-check the Azure portal.
- Teams chat IDs are opaque; the Graph API returns 404 if the service principal
  is not a member of the chat. Add the app to the conversation by sending a
  starter message via Graph Explorer first.
