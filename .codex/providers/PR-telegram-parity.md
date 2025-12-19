# Telegram provider parity dossier (host-era vs WASM component)

Status: **complete (code-referenced)**.

This dossier compares the current Telegram implementation in `greentic-messaging` (host-era: dedicated ingress/egress services + core renderer) with the new WASM provider component in `../greentic-messaging-providers`.

## Summary of current behavior (host-era)

### Formatting / rendering

- Adaptive Card → Telegram payload is produced by the Telegram renderer (`parse_mode: "HTML"` + `method: "sendMessage"` + `text`) in `libs/core/src/messaging_card/renderers/telegram.rs:18`.
- Enforces Telegram text limit via `TELEGRAM_TEXT_LIMIT` and truncation warning `"telegram.body_truncated"` in `libs/core/src/messaging_card/renderers/telegram.rs:117`.
- Escapes HTML and sanitizes tiered input via `sanitized_html(...)` and `html_escape(...)` in `libs/core/src/messaging_card/renderers/telegram.rs:192`.
- Converts actions into `reply_markup.inline_keyboard` with:
  - max 10 buttons (`MAX_BUTTONS`) and warning `"telegram.actions_truncated"` in `libs/core/src/messaging_card/renderers/telegram.rs:12`.
  - max 3 buttons per row (`MAX_PER_ROW`) in `libs/core/src/messaging_card/renderers/telegram.rs:13`.
  - `OpenUrl` → `{text,url}` and `Postback` → `{text,callback_data}` in `libs/core/src/messaging_card/renderers/telegram.rs:162`.
- Inputs are not supported; renderer inserts a “reply with your answer” prompt and emits warning `"telegram.inputs_not_supported"` in `libs/core/src/messaging_card/renderers/telegram.rs:68`.

### Egress sender behavior

- Secret lookup is tenant/team scoped via `messaging_credentials("telegram", ctx)` in `apps/egress-telegram/src/sender.rs:53`.
- Requires `channel` (Telegram `chat_id`) and either `text` or `payload` in `apps/egress-telegram/src/sender.rs:76`.
- Supports overriding Telegram Bot API method by accepting and removing `payload.method`; defaults to `"sendMessage"` in `apps/egress-telegram/src/sender.rs:156`.
- Ensures `payload.chat_id` and `payload.text` exist (if `text` provided) in `apps/egress-telegram/src/sender.rs:143`.
- Supports `mock://` api base for tests (no network) in `apps/egress-telegram/src/sender.rs:96`.
- Error handling:
  - non-2xx produces `telegram_send_failed` and adds retry backoff for 5xx in `apps/egress-telegram/src/sender.rs:115`.
  - extracts returned `message_id` from `result.message_id` in `apps/egress-telegram/src/sender.rs:129`.

### Ingress service behavior

- Public routes and admin endpoints:
  - Webhook route is secret-scoped: `/ingress/telegram/:tenant/:team/:secret` in `apps/ingress-telegram/src/main.rs:317`.
  - Admin operations: `/admin/telegram/{tenant}/register|deregister|status` in `apps/ingress-telegram/src/main.rs:322`.
- Validates webhook secret by comparing route `:secret` with stored `TelegramCreds.webhook_secret` in `apps/ingress-telegram/src/main.rs:490`.
- Parses incoming payload into `TelegramUpdate`, extracts `message` or `edited_message` in `apps/ingress-telegram/src/main.rs:402`.
- Threads: derives `thread_id` from `reply_to_message.message_id` or `message_thread_id` in `apps/ingress-telegram/src/main.rs:447`.
- Idempotency guard is applied per `(tenant, platform, msg_id)`; duplicates are dropped in `apps/ingress-telegram/src/main.rs:548`.
- Publishes an invocation envelope to NATS `greentic.msg.in.<tenant>.telegram.<chat_id>` in `apps/ingress-telegram/src/main.rs:583`.

### Provisioning

- On first ingress use per tenant/team, ingress calls `ensure_provisioned(...)` when `TelegramCreds.webhook_set` is false in `apps/ingress-telegram/src/main.rs:644`.
- Provisioning calls Telegram `setWebhook` using `TelegramCreds.bot_token` and persists `webhook_set=true` back to secrets in `libs/core/src/platforms/telegram/provision.rs:5`.

### Secrets schema expectations

- Host-era code expects `messaging/telegram.credentials.json` in tenant/team scope in `libs/core/src/secrets_paths.rs:12`.
- Current `TelegramCreds` is JSON with at least `bot_token`, `webhook_secret`, and `webhook_set` in `libs/core/src/platforms/telegram/creds.rs:4`.
- The pack manifest documents a JSON secret with `bot_token`, `chat_id`, and optional `secret_token` in `packs/messaging/components/telegram/component.manifest.json:1` (note: `chat_id`/`secret_token` are not part of `TelegramCreds` today, but are shown as examples).

### Capability metadata

- Declares `supports_threads: true`, `max_text_len: 4096`, and `rate_limit` in `providers/telegram/provider.json:1`.

## Summary of current behavior (WASM component model)

Component files in the new repo:

- WIT world exports: `send-message`, `handle-webhook`, `refresh`, `format-message` in `../greentic-messaging-providers/components/telegram/wit/telegram/world.wit:8`.
- Implementation:
  - `send_message` always calls Bot API `sendMessage` with `parse_mode: "HTML"` and secret `TELEGRAM_BOT_TOKEN` in `../greentic-messaging-providers/components/telegram/src/lib.rs:17`.
  - `handle_webhook` only parses JSON and wraps it as `{ok:true,event:<parsed>}` in `../greentic-messaging-providers/components/telegram/src/lib.rs:43`.
  - No webhook secret checks, allowed-update filtering, idempotency, or NATS publishing in component (by design; host responsibilities).
- Secrets requirements are token-only (`TELEGRAM_BOT_TOKEN`) in `../greentic-messaging-providers/components/telegram/component.manifest.json:1`.

## What belongs in WASM vs Host wrapper (ownership split)

### Host wrapper responsibilities (keep out of WASM)

- HTTP server, routing, and auth (webhook URL paths, admin endpoints).
- Idempotency store integration and NATS publishing (ingress pipeline).
- Telemetry exporter wiring and request/trace context propagation.
- `mock://` transports and integration-test plumbing.

Current host-owned examples:

- Telegram ingress routes + idempotency + NATS publish in `apps/ingress-telegram/src/main.rs:317`.
- Telegram egress HTTP calls and mock transport in `apps/egress-telegram/src/sender.rs:96`.

### WASM responsibilities (move/ensure in component)

- Formatting/rendering and limits enforcement (truncation + button limits).
- Webhook JSON parsing + normalization into a stable intermediate shape (not NATS publishing).
- Capability declaration (threads, limits, rate-limits) exposed to host.
- Validation of provider-specific invariants (e.g., required fields, allowed updates).

## API/WIT gaps needed for behavioral parity

The current Telegram WIT is too narrow to represent host-era capabilities:

1. **Threads/replies**
   - Host has `thread_id` semantics from `reply_to_message` / `message_thread_id` in `apps/ingress-telegram/src/main.rs:447`.
   - Proposed WIT gap: `format-message`/`send-message` should accept an optional `thread_id` and/or reply-to message id.

2. **Buttons / postbacks**
   - Host renderer supports inline keyboard `OpenUrl` + `Postback` in `libs/core/src/messaging_card/renderers/telegram.rs:143`.
   - Proposed WIT gap: formatting API should accept structured actions (or accept IR / card JSON) rather than raw `text`.

3. **Allowed update filtering**
   - Host `TelegramConfig.allowed_updates()` default includes `["message","callback_query"]` in `apps/ingress-telegram/src/config.rs:43`.
   - Proposed WIT gap: `handle-webhook` should be able to filter/validate update types and return a normalized “ignored” vs “accepted” response.

4. **Provisioning state**
   - Host persists `webhook_set` into secrets in `libs/core/src/platforms/telegram/provision.rs:5`.
   - Proposed WIT gap (optional): component could validate config and return desired webhook settings, but *state persistence + setWebhook call stays in host*.

## Secrets model mapping (host → WASM)

Host-era secret (tenant/team scoped JSON):

- Path: `secrets://<env>/<tenant>/<team>/messaging/telegram.credentials.json` via `messaging_credentials("telegram", ctx)` in `libs/core/src/secrets_paths.rs:12`.
- Type: `TelegramCreds { bot_token, webhook_secret, webhook_set }` in `libs/core/src/platforms/telegram/creds.rs:4`.

WASM token-only secret:

- Key: `TELEGRAM_BOT_TOKEN` in `../greentic-messaging-providers/components/telegram/component.manifest.json:1`.

Proposed mapping for parity:

- Keep host secret JSON as the canonical per-tenant configuration (bot token + webhook secret + provisioning status).
- When invoking component, host passes `chat_id` and `text` (and later thread/actions), and separately injects `TELEGRAM_BOT_TOKEN` into secrets-store for component calls. The bot token value can be sourced from `TelegramCreds.bot_token` (host) for now.
- Do **not** push webhook secrets into WASM; secret verification is performed by the host route layer already in `apps/ingress-telegram/src/main.rs:490`.

## Tests to preserve behavior

Formatting:

- Snapshot tests exist for Telegram translations in `libs/translator/tests/snapshots/snapshots__tg_text.snap:1` and `libs/translator/tests/snapshots/snapshots__tg_card.snap:1`.
- Renderer fixtures exist for Telegram in `libs/core/tests/fixtures/renderers/telegram/basic.json:1`.

Ingress normalization:

- Unit tests exist in `apps/ingress-telegram/src/main.rs:731` (module tests; add/update fixtures as needed).

Egress sender behavior:

- Unit tests for token caching and method extraction exist in `apps/egress-telegram/src/sender.rs:192`.

Contract/e2e:

- Telegram e2e exists (feature-gated) in `apps/egress-telegram/tests/e2e_telegram.rs:1`.

For WASM parity, add/retain:

- Golden tests for `format_message` matching current renderer output for representative cards (title/text/actions) (source of truth: `libs/core/src/messaging_card/renderers/telegram.rs:18`).
- Webhook fixture tests verifying `handle_webhook` emits consistent normalized shapes for `message`, `edited_message`, `callback_query` (current host parses only `message`/`edited_message` in `apps/ingress-telegram/src/main.rs:402`).

## Step-by-step thin-glue migration plan (host calling WASM later)

1. Define a “Telegram provider interface” in the host wrapper that consumes:
   - A common message-card/IR payload (not raw text) and returns Telegram-specific JSON (currently produced by `TelegramRenderer` in `libs/core/src/messaging_card/renderers/telegram.rs:18`).
2. Extend Telegram component WIT to accept a richer input:
   - Either message-card IR, or `{text, actions, parse_mode, thread_id}`.
3. In egress:
   - Replace `gsm_translator::TelegramTranslator` output usage with `wasm.format-message(...)` output (keep `reqwest` call and retries in host as in `apps/egress-telegram/src/sender.rs:103`).
4. In ingress:
   - Keep secret-scoped webhook route + idempotency + NATS publishing in host (`apps/ingress-telegram/src/main.rs:317`).
   - Delegate JSON parsing + normalization of updates into a minimal internal event object to `wasm.handle-webhook(...)`, then map that normalized object into `MessageEnvelope`.
5. Provisioning:
   - Keep calling Telegram `setWebhook` in host (currently `libs/core/src/platforms/telegram/provision.rs:5`), but allow component to validate config and return desired webhook properties (allowed updates, drop-pending).

