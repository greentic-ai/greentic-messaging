# PR-10B.md (greentic-messaging)
# Title: Provide pack/flow templates for provider-core messaging integration

## Goal
Add reusable flow/node templates that show how to call messaging providers via provider-core
WITHOUT hardcoding any specific provider.

## Non-goals
- Do not ship provider implementations here.
- Do not require runner changes in this PR.

## Deliverables
1) Example flow templates:
- `flows/messaging/send_message.flow.(json|yaml|gtc)` (whatever your flow format is)
  - uses ProviderInvoke node kind (from runner PR-07/08)
  - op: "send"
  - maps payload fields to send input JSON
  - writes output (message_id etc.) into state/payload

2) Docs:
- `docs/messaging_provider_core_flows.md`:
  - how to reference provider_id/provider_type
  - how to map input/output
  - how to select provider instance at runtime

3) Fixtures for integration repo (optional):
- small flow fixture used by greentic-integration PR-14.

## Acceptance criteria
- A future provider (e.g., WhatsApp) can be used by swapping provider_id/provider_type only.
