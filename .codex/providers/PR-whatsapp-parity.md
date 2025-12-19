# WhatsApp provider parity dossier (host-era vs WASM component)

Status: **initial draft (host-only; no WASM component found)**.

This dossier summarizes the WhatsApp implementation in `greentic-messaging` and notes the absence of a corresponding WASM provider component in `../greentic-messaging-providers`.

## Summary of current behavior (host-era)

- **Formatting / rendering**: WhatsApp renderer builds a text-first “template” payload with optional quick-reply / URL buttons (max 3, warning `whatsapp.actions_truncated`), downgrades inputs to instructional text (`whatsapp.inputs_not_supported`), enforces text limit (`WHATSAPP_TEXT_LIMIT`), and sanitizes URLs/text (`libs/core/src/messaging_card/renderers/whatsapp.rs`).
- **Egress sender behavior**: `WhatsAppSender` posts to Meta Graph `/messages` using `phone_id` and bearer `wa_user_token`, defaults to `https://graph.facebook.com/v19.0`, supports `mock://wa` for tests, extracts returned `messages[0].id`, and surfaces transport/retry hints for non-2xx (`libs/core/src/platforms/whatsapp/sender.rs`). `apps/egress-whatsapp/src/main.rs` wires NATS consumption, OAuth helper, and DLQ/backpressure.
- **Ingress service behavior**: `apps/ingress-whatsapp` handles Meta webhook verification (query `hub.verify_token` against secrets + HMAC signature using `app_secret`), subscribes to webhook events when missing (`libs/core/src/platforms/whatsapp/provision.rs`), normalizes message/postback events, enforces idempotency, and publishes envelopes to NATS.
- **Secrets schema expectations**: Tenant/team-scoped `messaging/whatsapp.credentials.json` containing `phone_id`, `wa_user_token`, `app_secret`, and `verify_token`; tracks `webhook_subscribed` and `subscription_signature` for provisioning (`libs/core/src/platforms/whatsapp/creds.rs`). Pack manifest documents the same key.
- **Capability metadata**: Declares template/text support, no threads, and rate limits in `providers/whatsapp/provider.json`.

## Summary of current behavior (WASM component model)

- No WhatsApp component or WIT world exists in `greentic-messaging-providers` today; host-only implementation.

## API/WIT gaps needed for behavioral parity

1. **Missing component** — All formatting, webhook handling, and transport live in host services.
2. **Formatting fidelity** — Component WIT would need to accept card IR (text, actions, inputs) and enforce WhatsApp limits/buttons exactly like `WhatsAppRenderer`, emitting warnings/metrics.
3. **Webhook verification** — Component would need access to headers/body for signature validation with `app_secret` and verify-token flow; current WIT shapes do not surface headers/query params.
4. **Session/template semantics** — Host controls message type (session text vs templates) and 24-hour window policy; deciding what (if any) should move into WASM remains open.
5. **Secrets mapping** — Need a contract for `PHONE_ID`, `WA_USER_TOKEN`, `APP_SECRET`, and `VERIFY_TOKEN` to match host scoping; current WIT assumes a single opaque secret.

## Tests to preserve behavior

- Renderer snapshots under `libs/core/tests/renderers_whatsapp_snapshot.rs` and fixtures in `libs/core/tests/fixtures/renderers/whatsapp/`.
- Ingress/unit tests in `apps/ingress-whatsapp/src/main.rs` modules (webhook verification, normalization, provisioning state).
- Egress sender tests using `mock://wa` base in `libs/core/src/platforms/whatsapp/sender.rs`; contract/e2e coverage in `apps/egress-whatsapp/tests/e2e_whatsapp.rs`.

## Step-by-step migration considerations

1. Keep webhook HTTP server, signature/verify-token checks, provisioning, idempotency, and NATS publishing in host.
2. Introduce a WASM `format-message` that mirrors `WhatsAppRenderer` output and warnings; optionally handle template vs session split.
3. If a WASM `handle-webhook` is desired, pass headers/body + query params and return normalized events matching current host shapes.
4. Define secret injection strategy for `phone_id`, `wa_user_token`, `app_secret`, and `verify_token` per tenant/team; align pack manifests/WIT accordingly.
5. Mirror existing renderer/ingress/e2e fixtures as goldens for any component outputs.
