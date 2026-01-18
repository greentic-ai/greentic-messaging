# 04-event-envelope-and-topics

## Envelope/message inventory (in-repo)

- `ChannelMessage` (normalized ingress payload used by gateway). Fields: `tenant`, `channel_id`, `session_id`, `route`, `payload`. (Evidence: `libs/core/src/types.rs:12-31`; `docs/audit/_evidence/rg/envelopes.txt`)
- `MessageEnvelope` (normalized inbound webhook message). Fields: `tenant`, `platform`, `chat_id`, `user_id`, `thread_id`, `msg_id`, `text`, `timestamp`, `context`. (Evidence: `libs/core/src/types.rs:94-126`; `docs/audit/_evidence/rg/envelopes.txt`)
- `InvocationEnvelope` (greentic-types): used as serialized envelope in NATS, conversion in `MessageEnvelope::into_invocation` and `TryFrom<InvocationEnvelope>`. (Evidence: `libs/core/src/types.rs:128-199`; `docs/audit/_evidence/rg/envelopes.txt`)
- `OutboundEnvelope` (generic outbound before channel-specific translation). Fields: `tenant`, `channel_id`, `session_id`, `meta`, `body`. (Evidence: `libs/core/src/outbound.rs:4-26`; `docs/audit/_evidence/rg/envelopes.txt`)
- `OutMessage` (runner emit to egress). Fields: `ctx`, `tenant`, `platform`, `chat_id`, `thread_id`, `kind`, `text`, `message_card`, `adaptive_card`, `meta`. (Evidence: `libs/core/src/types.rs:203-239`; `docs/audit/_evidence/rg/envelopes.txt`)
- `provider_ops::MessageEnvelope` alias (canonical ingest/output for providers): `ChannelMessageEnvelope` from `greentic-types`. (Evidence: `libs/core/src/provider_ops.rs:12-16`; `docs/audit/_evidence/rg/envelopes.txt`)
- `ActivitiesEnvelope` (webchat-specific ingress payload wrapper). (Evidence: `libs/core/src/platforms/webchat/ingress.rs:24-35`; `docs/audit/_evidence/rg/envelopes.txt`)
- `SlackEnvelope` (legacy Slack ingress envelope). (Evidence: `legacy/apps/ingress-slack/src/main.rs:97-110`; `docs/audit/_evidence/rg/envelopes.txt`)
- `ChangeEnvelope` (legacy Teams ingress envelope). (Evidence: `legacy/apps/ingress-teams/src/main.rs:106-140`; `docs/audit/_evidence/rg/envelopes.txt`)

## Serialization and conversion points

- `MessageEnvelope` <-> `InvocationEnvelope` conversion serializes payload and optional metadata (context). (Evidence: `libs/core/src/types.rs:128-199`; `docs/audit/_evidence/rg/envelopes.txt`)
- Gateway publishes `ChannelMessage` as JSON via `gsm_bus::to_value`. (Evidence: `apps/messaging-gateway/src/http.rs:248-275`; `crates/gsm-bus/src/lib.rs:53-63`; `docs/audit/_evidence/rg/envelopes.txt`)
- Egress publishes a simplified JSON payload (tenant/platform/chat_id/text/kind/metadata/adapter) rather than the full `OutMessage`. (Evidence: `apps/messaging-egress/src/main_logic.rs:175-190`; `docs/audit/_evidence/rg/envelopes.txt`)

## Topics and subject naming

- New gateway/egress subjects: `greentic.messaging.ingress.<env>.<tenant>.<team>.<platform>` and `greentic.messaging.egress.out.<tenant>.<platform>`. (Evidence: `crates/gsm-bus/src/lib.rs:33-63`; `apps/messaging-gateway/src/http.rs:35-43`; `docs/audit/_evidence/rg/envelopes.txt`)
- Legacy runner subjects: `greentic.msg.in.<tenant>.<platform>.<chat>` and `greentic.msg.out.<tenant>.<platform>.<chat>` via `in_subject`/`out_subject`. (Evidence: `libs/core/src/subjects.rs:20-49`; `tools/nats-demo/src/main.rs:27-61`; `docs/audit/_evidence/rg/envelopes.txt`)
- DLQ subjects: `dlq.{tenant}.{stage}` (configurable), replay subjects: `replay.{tenant}.{stage}`. (Evidence: `libs/dlq/src/lib.rs:32-141`; `docs/audit/_evidence/rg/state_store.txt`)

## Known Unknowns

- `InvocationEnvelope` and `ChannelMessageEnvelope` field definitions live in external crates, so the full schema is not visible here. (Evidence: `libs/core/src/types.rs:5-199`; `docs/audit/_evidence/cargo-metadata.json:1`)
