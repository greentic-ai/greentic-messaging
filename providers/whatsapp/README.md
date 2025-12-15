# WhatsApp Provider Quickstart

> Seed credentials via `greentic-secrets` (ctx + scaffold/wizard/apply). The env-var examples below are legacy and will be removed; prefer `greentic-secrets init --pack <pack>` with the messaging pack metadata.

The WhatsApp adapter targets the Meta Business Cloud API. The contract test sends
an interactive button message to a verified recipient and checks delivery status.

## Prerequisites

- A WhatsApp Business Account with Cloud API access.
- An application token (`WHATSAPP_TOKEN`) with `whatsapp_business_messaging`.
- A phone number ID representing your WhatsApp sender.
- A test recipient phone number that opted in to receive messages from the
  business (required for template sends).

## Configure secrets

Preferred (greentic-secrets):

```bash
greentic-secrets scaffold --pack fixtures/packs/messaging_secrets_smoke/pack.yaml --out /tmp/whatsapp-seed.yaml --env dev --tenant acme --team default
# Edit /tmp/whatsapp-seed.yaml to include:
# messaging/whatsapp.credentials.json:
#   token: EAAGm0PX4ZCpsBA...
#   phone_id: 123456789012345
#   recipient: 15551234567
greentic-secrets apply -f /tmp/whatsapp-seed.yaml
```

Legacy (deprecated) env setup:

```bash
export WHATSAPP_TOKEN=EAAGm0PX4ZCpsBA...
export WHATSAPP_PHONE_ID=123456789012345
export WHATSAPP_RECIPIENT=15551234567    # E.164 without plus sign
```

The contract test uses a stock approval-template message embedded in the repo.
Ensure the business account has the default “Utility” template approved, or
adjust the template inside the test before running it.

## Run the contract test

```bash
make stack-up
make conformance-whatsapp
make stack-down
```

The test sends an interactive button message, polls the Graph API for delivery
status, and prints the resulting state. Payloads and screenshots are written to
`tools/playwright/output/` and `target/e2e-artifacts/`.

## Troubleshooting

- A `400 (#100) Invalid parameter` error usually means the template or recipient
  has not been approved; double-check the template name and verify the phone
  number in Business Manager.
- Cloud API rate limits are strict; if you run multiple tests quickly, add a short
  delay between executions.
