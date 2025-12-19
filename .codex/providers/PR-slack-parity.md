# Slack provider parity dossier (host-era vs WASM component)

Status: **initial draft (code-referenced)**.

This dossier compares the current Slack implementation in `greentic-messaging` with the new WASM provider component in `../greentic-messaging-providers`.

## Summary of current behavior (host-era)

### Formatting / rendering

- Slack renderer supports:
  - Block kit payloads for “normal” cards and modal payloads when inputs are present in `libs/core/src/messaging_card/renderers/slack.rs:27`.
  - Tiered sanitization + Slack text truncation warnings (`"slack.text_truncated"`) in `libs/core/src/messaging_card/renderers/slack.rs:82`.
  - Headers (150 chars), modal title limit (24 chars), and action button limit (5) in `libs/core/src/messaging_card/renderers/slack.rs:11`.
  - FactSet becomes a `section` with `fields` (10 max) and warning `"slack.factset_truncated"` in `libs/core/src/messaging_card/renderers/slack.rs:124`.
  - Inputs:
    - When `include_inputs=false`, warns `"slack.inputs_require_modal"` in `libs/core/src/messaging_card/renderers/slack.rs:155`.
    - When `include_inputs=true`, produces Slack `input` blocks with `plain_text_input` or `static_select` in `libs/core/src/messaging_card/renderers/slack.rs:219`.
  - Actions:
    - `OpenUrl` → Slack button with `url` and `Postback` → button with JSON-stringified `value` in `libs/core/src/messaging_card/renderers/slack.rs:275`.

### Egress sender behavior

- The egress service reads JetStream, translates output into Slack payload(s) using `to_slack_payloads`, then calls `SlackSender` in `apps/egress-slack/src/main.rs:170`.
- Retry loop in the egress service attempts up to 3 times and uses backoff from `NodeError.backoff_ms` in `apps/egress-slack/src/main.rs:240`.
- `SlackSender`:
  - Resolves workspace credentials per tenant/team from:
    - a workspace index + per-workspace secrets (`slack.workspace.index.json` and `slack.workspace.<id>.json`) in `libs/core/src/platforms/slack/sender.rs:41`.
    - fallback to `slack.workspace.<team>.json` in `libs/core/src/platforms/slack/sender.rs:56`.
  - Uses `bearer_auth` with `SlackWorkspace.bot_token` and calls `chat.postMessage` in `libs/core/src/platforms/slack/sender.rs:142`.
  - Treats `429` and 5xx as retryable and honors `retry-after` header in `libs/core/src/platforms/slack/sender.rs:152`.
  - Supports `mock://` base for CI/tests in `libs/core/src/platforms/slack/sender.rs:122` and `apps/egress-slack/src/main.rs:49`.

### Ingress service behavior

- Ingress endpoint: `/ingress/slack/:tenant/:team/events` in `apps/ingress-slack/src/main.rs:70`.
- Handles Slack URL verification by returning `challenge` in `apps/ingress-slack/src/main.rs:232`.
- Verifies Slack signature using `X-Slack-Request-Timestamp` and `X-Slack-Signature` in `apps/ingress-slack/src/main.rs:344`.
- Filters event types:
  - Drops bot events (`bot_id`) and specific subtypes (`bot_message`, `message_changed`, `message_deleted`) in `apps/ingress-slack/src/main.rs:260`.
  - Only processes `message` events, otherwise drops in `apps/ingress-slack/src/main.rs:283`.
  - Sets `thread_id` from `thread_ts` in `apps/ingress-slack/src/main.rs:108` and `apps/ingress-slack/src/main.rs:302`.
- Applies idempotency and publishes to NATS in `apps/ingress-slack/src/main.rs:159`.

### Provisioning / OAuth

- Slack OAuth handler exists as a separate app and stores per-workspace credentials under secrets paths:
  - uses `slack_workspace_secret` + `slack_workspace_index` in `apps/slack_oauth/src/main.rs:50`.
  - reads Slack OAuth client env vars (`SLACK_CLIENT_ID`, `SLACK_CLIENT_SECRET`, etc.) in `apps/slack_oauth/src/main.rs:93`.

### Secrets schema expectations

- Host-era Slack egress expects `SlackWorkspace { workspace_id, bot_token }` in `libs/core/src/platforms/slack/workspace.rs:3`.
- Host-era “generic” messaging creds path helper still exists (`messaging/slack.credentials.json`) but SlackSender does **not** use it; it uses `slack.workspace.*` secrets via `slack_workspace_secret(...)` in `libs/core/src/secrets_paths.rs:22`.
- Pack manifest documents a single JSON secret `messaging/slack.credentials.json` with `bot_token`, optional `signing_secret`, and optional `channel_id` in `packs/messaging/components/slack/component.manifest.json:1` (note: not aligned with SlackSender’s workspace index approach).

### Capability metadata

- Declares `supports_threads: true`, `attachments: true`, `max_text_len: 40000`, and Slack rate-limits in `providers/slack/provider.json:1`.

## Summary of current behavior (WASM component model)

- WIT world exports:
  - `init-runtime-config`, `send-message`, `handle-webhook`, `refresh`, `format-message` in `../greentic-messaging-providers/components/slack/wit/slack/world.wit:8`.
- Implementation:
  - `send_message` always posts to `https://slack.com/api/chat.postMessage` with an `Authorization: Bearer <SLACK_BOT_TOKEN>` header in `../greentic-messaging-providers/components/slack/src/lib.rs:31`.
  - `format_message` returns a JSON payload containing `{channel,text,blocks:[section(mrkdwn)]}` in `../greentic-messaging-providers/components/slack/src/lib.rs:103`.
  - `handle_webhook` optionally verifies Slack signature using `SLACK_SIGNING_SECRET` if present in secrets-store in `../greentic-messaging-providers/components/slack/src/lib.rs:56`.
  - `handle_webhook` does not extract/normalize Slack events (it wraps the parsed JSON body as `event`) in `../greentic-messaging-providers/components/slack/src/lib.rs:66`.
- Component secrets are token + optional signing secret in `../greentic-messaging-providers/components/slack/component.manifest.json:11`.

## What belongs in WASM vs Host wrapper (ownership split)

### Host wrapper responsibilities (keep out of WASM)

- Slack ingress HTTP server, URL verification, idempotency, NATS publishing in `apps/ingress-slack/src/main.rs:70`.
- Slack OAuth flow and secret persistence (`apps/slack_oauth/...`), including workspace indexing in `apps/slack_oauth/src/main.rs:50`.
- Transport (`reqwest`), retries, and `mock://` wiring in `libs/core/src/platforms/slack/sender.rs:122` and `apps/egress-slack/src/main.rs:49`.

### WASM responsibilities (move/ensure in component)

- Formatting (Block Kit and modal rendering) + limit enforcement + warning/metrics hooks (currently in `libs/core/src/messaging_card/renderers/slack.rs:27`).
- Webhook parsing/normalization:
  - Interpret Slack Events API envelopes and reduce to normalized “message event” objects, preserving `channel`, `user`, `ts`, `thread_ts` and filtering bot/subtype events (currently in `apps/ingress-slack/src/main.rs:254`).
- Capability declaration (threads + limits) based on `providers/slack/provider.json:1`.

## API/WIT gaps needed for behavioral parity

1. **Formatting input is too narrow**
   - Host renderer expects full message-card IR with elements/actions/inputs (`MessageCardIr`) in `libs/core/src/messaging_card/renderers/slack.rs:27`.
   - Component only accepts `(channel,text)` in `../greentic-messaging-providers/components/slack/wit/slack/world.wit:15`.
   - Gap: WIT should accept a richer structure (IR or card JSON + actions/inputs) and expose “modal vs blocks” output.

2. **Webhook normalization is missing**
   - Host filters by event type/subtype/bot fields and maps to internal envelope in `apps/ingress-slack/src/main.rs:254`.
   - Component currently just wraps parsed JSON as `{ok:true,event:<body>}` in `../greentic-messaging-providers/components/slack/src/lib.rs:66`.
   - Gap: `handle-webhook` should return a normalized object (or a list of events) ready for host mapping to `MessageEnvelope`.

3. **Secrets model mismatch**
   - Host uses workspace-indexed secrets (`slack.workspace.*`) in `libs/core/src/platforms/slack/sender.rs:41`.
   - Component expects a single `SLACK_BOT_TOKEN` secret in `../greentic-messaging-providers/components/slack/component.manifest.json:11`.
   - Gap: define how host supplies the correct workspace token to WASM (per team/workspace).

## Secrets model mapping (host → WASM)

Current host secret sources:

- Workspace token is read from `slack.workspace.<workspace_id>.json` and selected via `slack.workspace.index.json` in `libs/core/src/platforms/slack/sender.rs:41`.

Component inputs:

- `SLACK_BOT_TOKEN` in `../greentic-messaging-providers/components/slack/component.manifest.json:11`.
- Optional `SLACK_SIGNING_SECRET` for webhook verification in `../greentic-messaging-providers/components/slack/component.manifest.json:11`.

Proposed mapping for parity:

- Keep workspace indexing in host (OAuth stores these secrets already in `apps/slack_oauth/src/main.rs:50`).
- At call time, host resolves the `SlackWorkspace.bot_token` and injects it into the WASM secrets-store as `SLACK_BOT_TOKEN` for that invocation.
- Decide whether Slack signing secret is global-per-tenant or per-workspace; host ingress currently uses a single env var `SLACK_SIGNING_SECRET` in `apps/ingress-slack/src/main.rs:54` (component assumes secret-store).

## Tests to preserve behavior

Formatting:

- Snapshot tests exist for Slack renderer in `libs/core/tests/renderers_slack_snapshot.rs:1`.

Ingress parsing:

- Signature verification tests in `apps/ingress-slack/src/main.rs:395`.
- Event filtering tests in `apps/ingress-slack/src/main.rs:443`.

WASM parity tests to add/retain:

- Golden formatting tests for complex cards: headers, fact sets, actions, and modal inputs (source of truth currently in `libs/core/src/messaging_card/renderers/slack.rs:50`).
- Webhook fixture tests verifying bot/subtype filtering and extracted fields match `slack_event_invocation(...)` behavior in `apps/ingress-slack/src/main.rs:254`.

## Step-by-step thin-glue migration plan (host calling WASM later)

1. Define a Slack WASM-facing “format” call that accepts message-card IR (or equivalent) and returns either:
   - a block payload, or
   - a modal payload + `used_modal` signal (currently `RenderOutput.used_modal` in `libs/core/src/messaging_card/renderers/slack.rs:40`).
2. Egress:
   - Keep `SlackSender` and HTTP call in host (`libs/core/src/platforms/slack/sender.rs:109`).
   - Replace `to_slack_payloads` output generation with `wasm.format-message(...)` output.
3. Ingress:
   - Keep URL verification, signature verification, idempotency, and NATS publishing in host (`apps/ingress-slack/src/main.rs:128`).
   - Delegate body parsing + event normalization to WASM (`handle-webhook`), then map normalized events to `MessageEnvelope`.

