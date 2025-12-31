# Provider-core messaging operations

`greentic-messaging` hosts the canonical JSON Schemas and serde types for messaging providers. Use these contracts when implementing adapters or validating payloads across providers.

- Schemas live under `schemas/messaging/**`.
- Rust types live in `gsm-core::provider_ops` and align with the schemas.
- Fixture examples: `tests/fixtures/send_input.json`, `tests/fixtures/send_output.json`.

## Operation names

- `send`: deliver a message to a channel/recipient.
- `reply`: respond to an existing message id (provider or Greentic id).
- `ingest`: normalize an incoming webhook into the canonical `MessageEnvelope`.

## Message envelope

- Canonical envelope: `schemas/messaging/common/message_envelope.schema.json` (mirrors `greentic_types::ChannelMessageEnvelope`).
- Rust alias: `gsm_core::ProviderMessageEnvelope` (`greentic_types::ChannelMessageEnvelope`).
- Fields: `id`, `tenant` (`TenantCtx`), `channel`, `session_id`, optional `user_id`, optional `text`, `attachments` (`mime_type`, `url`, optional name/size), `metadata` (string map).

## send (provider-core)

Input schema: `schemas/messaging/ops/send.input.schema.json`

```json
{
  "to": "string",                        // channel/room/address
  "text": "string?",                     // optional text, trimmed
  "attachments": [                       // optional base64 attachments
    { "name": "...", "content_type": "...", "data_base64": "..." }
  ],
  "metadata": {
    "thread_id": "string?",
    "reply_to": "string?",
    "tags": ["..."]
  }
}
```

Output schema: `schemas/messaging/ops/send.output.schema.json`

```json
{
  "message_id": "string",              // Greentic-level id (uuid ok)
  "provider_message_id": "string?",    // provider native id
  "thread_id": "string?",
  "status": "sent|queued",
  "ts": "RFC3339 timestamp?"
}
```

Rust types: `SendInput`, `SendOutput`, `SendStatus`, `SendMetadata`, `AttachmentInput`. Validation helper: `validate_send_input(&SendInput)`.

## reply (provider-core)

- Input schema: `schemas/messaging/ops/reply.input.schema.json` (requires `reply_to` and `text` or `attachments`).
- Output schema: `schemas/messaging/ops/reply.output.schema.json` (alias of send output).
- Rust types: `ReplyInput`, `ReplyMetadata`, `ReplyOutput`. Validation helper: `validate_reply_input(&ReplyInput)`.

## ingest (provider-core)

- Input schema: `schemas/messaging/ops/ingest.input.schema.json` (`provider_type`, `payload`, optional `tenant`, `headers`, `received_at`).
- Output schema: `schemas/messaging/ops/ingest.output.schema.json` (refers to `MessageEnvelope`).
- Rust types: `IngestInput`, `IngestOutput`, `ProviderMessageEnvelope`. Normalizer helper: `normalize_envelope(&mut ProviderMessageEnvelope)` trims text/metadata.

## Provider type naming

- Prefer dotted identifiers: `messaging.telegram.bot`, `messaging.teams.bot`, `messaging.slack.app`, `messaging.webchat.default`.
- Keep the `messaging.` prefix for channel providers to avoid collisions with other domains.

## Minimal requirements per op

- `send`: `to` plus (`text` or attachment). Surface `thread_id` when available; emit `message_id` and `status` in output.
- `reply`: add `reply_to` (provider or Greentic id) and preserve threading metadata when present.
- `ingest`: return a normalized `MessageEnvelope` with `id`, `tenant`, `channel`, `session_id`, and `metadata` (set empty values to `null`/omit after calling `normalize_envelope`).
