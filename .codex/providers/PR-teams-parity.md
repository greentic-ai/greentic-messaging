# Teams provider parity dossier (host-era vs WASM component)

Status: **initial draft (code-referenced)**.

This dossier compares the current Microsoft Teams implementation in `greentic-messaging` with the new WASM provider component in `../greentic-messaging-providers`.

## Summary of current behavior (host-era)

### Formatting / rendering

- Teams renderer outputs an Adaptive Card “attachment” payload via `adaptive_from_ir(...)` in `libs/core/src/messaging_card/renderers/teams.rs:20`.
- OAuth auth-card rendering exists separately (`application/vnd.microsoft.card.oauth`) in `libs/core/src/messaging_card/renderers/teams.rs:32`.

### Egress sender behavior

- Egress service uses `TeamsSender` to deliver messages and supports OAuth-card downgrade/fallback in `apps/egress-teams/src/main.rs:211`.
- Graph base URLs can be overridden or mocked:
  - auth base env: `MS_GRAPH_AUTH_BASE` or `mock://auth` in `apps/egress-teams/src/main.rs:37`.
  - api base env: `MS_GRAPH_API_BASE` or `mock://graph` in `apps/egress-teams/src/main.rs:42`.
- `TeamsSender`:
  - Uses tenant/team-scoped `messaging/teams.credentials.json` for client credentials in `libs/core/src/platforms/teams/sender.rs:61`.
  - Requires a conversation mapping (logical `channel` → Teams chat id) from `messaging/teams.conversations.json` in `libs/core/src/platforms/teams/sender.rs:72`.
  - Fetches a Graph token via client credentials grant in `libs/core/src/platforms/teams/sender.rs:100`.
  - Sends messages to `POST /chats/{chat_id}/messages` in `libs/core/src/platforms/teams/sender.rs:95`.
  - Builds payloads:
    - card attachment wrapper in `libs/core/src/platforms/teams/sender.rs:149`
    - or plain text payload in `libs/core/src/platforms/teams/sender.rs:165`.

### Ingress service behavior

- Ingress exposes `/teams/webhook`:
  - `GET` validation echoes `validationToken` in `apps/ingress-teams/src/main.rs:78`.
  - `POST` accepts a Graph change notification envelope and publishes one invocation per notification in `apps/ingress-teams/src/main.rs:154`.
- Optional bearer auth (env `INGRESS_BEARER`) enforced in `apps/ingress-teams/src/main.rs:159`.
- Derives `chat_id` from Graph resource string and stores raw `resource_data` in message context:
  - `extract_chat_id` in `apps/ingress-teams/src/main.rs:111`
  - `context` in `apps/ingress-teams/src/main.rs:115`.
- Applies idempotency per `teams:<resource_data.id>` in `apps/ingress-teams/src/main.rs:184`.
- Publishes to NATS via `in_subject(...)` in `apps/ingress-teams/src/main.rs:183`.

### Provisioning (subscriptions)

- There is a dedicated Teams subscriptions service that manages Microsoft Graph subscriptions over NATS:
  - subscribes to admin subject `greentic.subs.admin.<tenant>.teams` in `apps/subscriptions-teams/src/main.rs:41`.
  - issues `POST https://graph.microsoft.com/v1.0/subscriptions` with `notificationUrl` and `expirationDateTime` in `apps/subscriptions-teams/src/main.rs:101`.

### Secrets schema expectations

- Credentials are a tenant/team scoped JSON:
  - `messaging/teams.credentials.json` via `messaging_credentials("teams", ctx)` in `libs/core/src/secrets_paths.rs:12`.
  - Type `TeamsCredentials { tenant_id, client_id, client_secret }` in `libs/core/src/platforms/teams/sender.rs:12`.
- Conversation mapping is a tenant/team scoped JSON:
  - `messaging/teams.conversations.json` via `teams_conversations_secret(ctx)` in `libs/core/src/secrets_paths.rs:43`.
  - Type `TeamsConversations { items: HashMap<String, TeamsConversation> }` in `libs/core/src/platforms/teams/conversations.rs:24`.
- Pack manifest documents only the credentials JSON in `packs/messaging/components/teams/component.manifest.json:1` (no mention of conversations mapping).

### Capability metadata

- Declares `supports_threads: true`, `attachments: true`, `max_text_len: 28000`, and rate-limits in `providers/teams/provider.json:1`.

## Summary of current behavior (WASM component model)

- WIT world exports:
  - `init-runtime-config`, `send-message`, `handle-webhook`, `refresh`, `format-message` in `../greentic-messaging-providers/components/teams/wit/teams/world.wit:8`.
- Implementation:
  - `send_message(destination_json,text)`:
    - expects `destination_json` containing `{team_id, channel_id}` in `../greentic-messaging-providers/components/teams/src/lib.rs:150`.
    - posts to `POST https://graph.microsoft.com/v1.0/teams/<team_id>/channels/<channel_id>/messages` in `../greentic-messaging-providers/components/teams/src/lib.rs:36`.
  - Obtains token by calling `https://login.microsoftonline.com/<tenant>/oauth2/v2.0/token` using secrets-store keys `MS_GRAPH_TENANT_ID`, `MS_GRAPH_CLIENT_ID`, `MS_GRAPH_CLIENT_SECRET` in `../greentic-messaging-providers/components/teams/src/lib.rs:97`.
  - `handle_webhook` wraps parsed JSON as `{ok:true,event:<parsed>}` in `../greentic-messaging-providers/components/teams/src/lib.rs:64`.
- Component secret requirements list the three MS Graph secrets in `../greentic-messaging-providers/components/teams/component.manifest.json:11`.

## What belongs in WASM vs Host wrapper (ownership split)

### Host wrapper responsibilities (keep out of WASM)

- HTTP ingress server + idempotency + NATS publishing in `apps/ingress-teams/src/main.rs:154`.
- Subscription management / renewal loop in `apps/subscriptions-teams/src/main.rs:45`.
- `mock://` transports and `reqwest` integration for Graph calls (currently in `apps/egress-teams/src/main.rs:37` and `libs/core/src/platforms/teams/sender.rs:185`).

### WASM responsibilities (move/ensure in component)

- Formatting and provider limits enforcement (card formatting already exists in host renderer in `libs/core/src/messaging_card/renderers/teams.rs:20`).
- Webhook parsing/normalization of Graph change notifications into stable internal event shapes (currently handled directly in host in `apps/ingress-teams/src/main.rs:170`).
- Capability declaration (threads/limits) based on `providers/teams/provider.json:1`.

## API/WIT gaps needed for behavioral parity

1. **Destination mismatch (chat vs channel)**
   - Host egress sends to Graph chats: `POST /chats/{chat_id}/messages` in `libs/core/src/platforms/teams/sender.rs:95`.
   - Component sends to Teams channels: `POST /teams/{team_id}/channels/{channel_id}/messages` in `../greentic-messaging-providers/components/teams/src/lib.rs:36`.
   - Gap: WIT should support the host-era “conversation/chat” destination model (or host needs to adopt channel-based sends).

2. **Conversation mapping is host-only today**
   - Host requires `messaging/teams.conversations.json` mapping in `libs/core/src/platforms/teams/sender.rs:72`.
   - Component has no notion of the mapping; it requires explicit `{team_id, channel_id}` destination JSON in `../greentic-messaging-providers/components/teams/src/lib.rs:150`.
   - Gap: define what the canonical destination format is and where mapping lives (host vs component).

3. **Webhook normalization**
   - Host currently parses `ChangeEnvelope { value: [...] }` directly in `apps/ingress-teams/src/main.rs:170`.
   - Component returns only `{ok:true,event:<parsed>}` in `../greentic-messaging-providers/components/teams/src/lib.rs:64`.
   - Gap: component should parse and normalize change notifications (extract resource/chat id, resourceData.id), leaving host to do idempotency + NATS publish.

4. **Formatting API is narrow**
   - Host renderer accepts `MessageCardIr` and produces Adaptive Card payload via `adaptive_from_ir` in `libs/core/src/messaging_card/renderers/teams.rs:20`.
   - Component `format-message` accepts `(destination_json,text)` in `../greentic-messaging-providers/components/teams/wit/teams/world.wit:15`.
   - Gap: expose richer formatting input (IR/card) and output the final Graph message JSON shape that host expects (`build_card_payload`) in `libs/core/src/platforms/teams/sender.rs:149`.

## Secrets model mapping (host → WASM)

Host-era secrets:

- `messaging/teams.credentials.json` (JSON) in `libs/core/src/platforms/teams/sender.rs:61`.
- `messaging/teams.conversations.json` (JSON map) in `libs/core/src/platforms/teams/sender.rs:72`.

Component secrets:

- `MS_GRAPH_TENANT_ID`, `MS_GRAPH_CLIENT_ID`, `MS_GRAPH_CLIENT_SECRET` in `../greentic-messaging-providers/components/teams/component.manifest.json:11`.

Proposed mapping for parity:

- Keep credentials canonical in host JSON (`TeamsCredentials`) and expose them to component via secrets-store keys (or map them inside host).
- Keep conversation mapping in host (it is runtime/environment specific) and pass a normalized “destination” object to WASM for formatting only.

## Tests to preserve behavior

Formatting:

- Teams renderer behavior is exercised through egress rendering paths; render calls are invoked in `apps/egress-teams/src/main.rs:255` (see `render_adaptive_card`).

Ingress parsing:

- Ingress has unit tests asserting envelope shapes in `apps/ingress-teams/src/main.rs:286`.

WASM parity tests to add/retain:

- Golden tests for webhook normalization: `ChangeEnvelope` with multiple notifications should yield stable normalized events (source behavior in `apps/ingress-teams/src/main.rs:170`).
- Golden tests for “chat message” vs “channel message” destination shape decision (must align with chosen host behavior).

## Step-by-step thin-glue migration plan (host calling WASM later)

1. Decide destination model:
   - If host keeps chat-based messaging, extend Teams component to support chat destinations (e.g. `{chat_id}`) and produce Graph `/chats/{chat_id}/messages` payloads.
2. Define normalized webhook event shape in WIT:
   - Parse Graph `ChangeEnvelope` and return an array of normalized events (host loops and publishes).
3. Egress:
   - Keep token acquisition, retries, and HTTP send in host (`TeamsSender` in `libs/core/src/platforms/teams/sender.rs:185`).
   - Replace `TeamsRenderer` usage with `wasm.format-message(...)` output once WIT accepts rich inputs.
4. Ingress:
   - Keep bearer auth and validation token handling in host (`apps/ingress-teams/src/main.rs:84`).
   - Delegate JSON parsing + normalization to WASM, then map normalized output into `MessageEnvelope` and publish to NATS.

