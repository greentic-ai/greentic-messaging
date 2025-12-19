# Provider parity index (host-era vs WASM)

This index tracks behavioral parity between `greentic-messaging` (host-era provider implementations) and `greentic-messaging-providers` (WASM provider components).

Legend:

- **Parity status**: `✅` (close), `🟡` (partial), `🔴` (major gaps), `—` (not assessed).
- **Effort**: `S` / `M` / `L` to reach “behaviorally equivalent formatting + normalization” under the rule: ingress servers, NATS publishing, idempotency, telemetry exporters, and `mock://` transports remain host responsibilities.

| Provider | Dossier | Parity status | Key gaps (WASM vs host-era) | Effort | Dependencies |
|---|---|---:|---|---:|---|
| Telegram | `.codex/providers/PR-telegram-parity.md` | 🟡 | No thread/reply modeling; no action/button rendering; webhook normalization is shallow | M | WIT extensions for actions + thread_id; agreed normalized webhook shape |
| Slack | `.codex/providers/PR-slack-parity.md` | 🔴 | Formatting is minimal (no modal/limits/actions); webhook normalization missing; secrets model mismatch (workspace tokens) | L | WIT for card/IR formatting; normalized event output; host token injection strategy |
| Teams | `.codex/providers/PR-teams-parity.md` | 🔴 | Destination mismatch (host uses chats, WASM uses channels); webhook normalization missing; formatting input too narrow | L | Decide chat vs channel; WIT for normalized change notifications; formatting WIT changes |
| Webchat | `.codex/providers/PR-webchat-parity.md` | 🔴 | No WASM component; Direct Line gateway (tokens/sessions) is host-only; formatting in renderer only | L | Decide if WebChat stays host-only; WIT shape for Direct Line + signing secrets |
| Webex | `.codex/providers/PR-webex-parity.md` | 🔴 | No WASM component; webhook signature + provisioning are host-only; formatting requires full card IR | M | Define Webex WIT (format + normalize); header-aware signatures; secret injection |
| WhatsApp | `.codex/providers/PR-whatsapp-parity.md` | 🔴 | No WASM component; webhook verification and Graph transport are host-only; formatting needs IR + warnings | M | WIT for IR formatting; webhook headers/query; multi-secret mapping |
