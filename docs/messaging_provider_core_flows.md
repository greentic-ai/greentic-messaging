# Provider-core messaging flow templates

This repo includes a flow template that calls messaging providers via the provider-core `ProviderInvoke` node (runner PR-07/08). Swap providers by changing `provider_id` or `provider_type`; no flow logic changes are required.

## Template: `flows/messaging/send_message.ygtc`

- Node kind: `provider_invoke` (ProviderInvoke).
- Op: `send`.
- Input mapping:
  - `to`, `text`, `attachments`, `metadata.thread_id`, `metadata.reply_to`, `metadata.tags` pulled from `payload`.
- Output mapping:
  - Writes `result.message_id`, `provider_message_id`, `thread_id`, `status`, `ts` back into `payload` (`payload.sent_at` mirrors `result.ts`).

### Runtime selection of providers

- `provider_id`: concrete instance (e.g., `messaging.telegram.bot-main`). Recommended when multiple instances of the same provider type exist.
- `provider_type`: dotted type name (e.g., `messaging.whatsapp.bot`). Useful when routing tables resolve to instances at runtime.
- You can parameterize either via `params.provider_id` / `params.provider_type` to let callers choose at invocation time.

### Fixtures

- `fixtures/flows/provider_core_send.ygtc`: Minimal send flow used by integration tests (same mapping as the template, default provider type set to `messaging.local.mock`).

### Example invocation context

```json
{
  "params": {
    "provider_type": "messaging.whatsapp.bot"
  },
  "payload": {
    "to": "chat-123",
    "text": "Hello from provider-core",
    "thread_id": "thread-42",
    "tags": ["support"]
  }
}
```

### Notes

- The template relies on provider-core contracts defined in `schemas/messaging/ops/send.*.schema.json` and serde types in `gsm_core::provider_ops`.
- Outputs from the provider invoke are left in `payload` for downstream nodes to use (logging, persistence, branching).
- To switch providers, update `provider_id`/`provider_type` only; the rest of the flow remains unchanged.
