# PR-10A.md (greentic-messaging)
# Title: Define provider-core messaging operation contracts (JSON schemas + shared types)

## Goal
Make `greentic-messaging` the authoritative place for:
- Messaging envelope JSON shapes (aligned with greentic-types messaging envelope)
- Provider-core operation contracts (input/output JSON schemas for ops like send/reply/ingest)
- Shared helpers to validate/normalize messages across providers
WITHOUT introducing provider-specific code or new WIT protocols.

## Non-goals
- Do not implement Telegram/Teams/SMTP/etc. here.
- Do not depend on greentic-interfaces-guest; use greentic-types + JSON schema only.
- Do not introduce a second provider mechanism.

## Deliverables
1) Canonical JSON Schemas for messaging provider-core ops:
- `schemas/messaging/common/message_envelope.schema.json`
- `schemas/messaging/ops/send.input.schema.json`
- `schemas/messaging/ops/send.output.schema.json`
- `schemas/messaging/ops/reply.input.schema.json` (optional)
- `schemas/messaging/ops/reply.output.schema.json` (optional)
- `schemas/messaging/ops/ingest.input.schema.json` (optional)
- `schemas/messaging/ops/ingest.output.schema.json` (optional)

2) Rust types (serde) matching those schemas (optional but recommended):
- `src/provider_ops.rs`:
  - `SendInput`, `SendOutput`, `ReplyInput`, `ReplyOutput`, `IngestInput`, `IngestOutput`
  - `MessageEnvelope` (or reuse from greentic-types via newtype wrapper)

3) Validation helpers:
- `src/validate.rs`:
  - `validate_send_input(&SendInput) -> Result<(), ValidationIssue>`
  - `normalize_envelope(...)` (optional)

4) Docs:
- `docs/provider_core_messaging_ops.md` describing:
  - op names (send/reply/ingest)
  - minimal required fields
  - recommended provider_type naming (messaging.telegram.bot etc.)
  - output contract (message_id, provider_message_id, thread_id, timestamps)

## Operation contracts (minimum)
### invoke("send")
Input JSON:
{
  "to": "string",
  "text": "string",
  "attachments": [ { "name": "...", "content_type": "...", "data_base64": "..." } ]?,
  "metadata": { "thread_id": "...", "reply_to": "...", "tags": ["..."] }?
}

Output JSON:
{
  "message_id": "string",              // Greentic-level id (can be uuid)
  "provider_message_id": "string"?,    // provider native id
  "thread_id": "string"?,
  "status": "sent|queued",
  "ts": "RFC3339 timestamp"?
}

### invoke("ingest") (optional)
Input: raw provider webhook payload + hints
Output: normalized `MessageEnvelope` JSON

## Tests
- JSON schema validation tests (if you have schema tooling):
  - each schema is valid JSON schema
  - example fixtures validate against schemas
- Serde round-trip tests for Rust types (if you add them)
- Golden example fixtures:
  - `tests/fixtures/send_input.json`
  - `tests/fixtures/send_output.json`

## Acceptance criteria
- Schemas exist and are stable.
- Providers can implement send/reply/ingest ops without guessing payload shape.
- No provider-specific dependencies added.
