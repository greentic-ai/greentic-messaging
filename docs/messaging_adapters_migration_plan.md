# Messaging adapters migration plan (pack-based)

This maps existing adapters to pack-defined `messaging.adapters` entries. No runtime changes yet; this is planning only.

Pack/Component convention: for each row, `pack_name` is a pack YAML (e.g., `packs/messaging/slack.yaml`, logical id `greentic-messaging-slack`), and `component` is the logical ID of the WASM component that implements that adapter (e.g., `slack-adapter@1.0.0`).

| adapter_name | pack_name | messaging.adapters entry (suggested) | default_flow | custom_flow | capabilities (examples) |
| --- | --- | --- | --- | --- | --- |
| slack | `greentic-messaging-slack` | name: `slack-main`, kind: `ingress-egress`, component: `slack-adapter@1.0.0` | `flows/messaging/slack/default.ygtc` | `flows/messaging/slack/custom.ygtc` | direction: ingress+egress; features: threads, attachments, reactions |
| teams | `greentic-messaging-teams` | name: `teams-main`, kind: `ingress-egress`, component: `teams-adapter@1.0.0` | `flows/messaging/teams/default.ygtc` | `flows/messaging/teams/custom.ygtc` | direction: ingress+egress; features: threads, attachments, cards (adaptive), reactions |
| webex | `greentic-messaging-webex` | name: `webex-main`, kind: `ingress-egress`, component: `webex-adapter@1.0.0` | `flows/messaging/webex/default.ygtc` | `flows/messaging/webex/custom.ygtc` | direction: ingress+egress; features: threads (reply), attachments, reactions |
| webchat | `greentic-messaging-webchat` | name: `webchat-main`, kind: `ingress-egress`, component: `webchat-adapter@1.0.0` | `flows/messaging/webchat/default.ygtc` | `flows/messaging/webchat/custom.ygtc` | direction: ingress+egress; features: attachments, simple threads |
| whatsapp | `greentic-messaging-whatsapp` | name: `whatsapp-main`, kind: `ingress-egress`, component: `whatsapp-adapter@1.0.0` | `flows/messaging/whatsapp/default.ygtc` | `flows/messaging/whatsapp/custom.ygtc` | direction: ingress+egress; features: text, media attachments, no threads |
| telegram (ingress) | `greentic-messaging-telegram` | name: `telegram-ingress`, kind: `ingress`, component: `telegram-ingress-adapter@1.0.0` | `flows/messaging/telegram/ingress_default.ygtc` | `flows/messaging/telegram/ingress_custom.ygtc` | direction: ingress; features: threads (reply), attachments minimal |
| telegram (egress) | `greentic-messaging-telegram` | name: `telegram-egress`, kind: `egress`, component: `telegram-egress-adapter@1.0.0` | `flows/messaging/telegram/egress_default.ygtc` | `flows/messaging/telegram/egress_custom.ygtc` | direction: egress; features: text, cards (rendered), no threads beyond reply_to |
| local (mock/dev) | `greentic-messaging-local` | name: `local-main`, kind: `ingress-egress`, component: `local-adapter@1.0.0` | `flows/messaging/local/default.ygtc` | `flows/messaging/local/custom.ygtc` | direction: ingress+egress; features: text-only, logs to stdout/file; no external deps |

Notes:
- Schema: extend pack v1 `messaging.adapters` (optional) mirroring `events.providers`; keep existing configs working until migrated.
- Components: identifiers above are logical stubs to be provided by greentic-pack defaults; greentic-messaging should load them via greentic-runner/greentic-interfaces host bindings.
- Flows: paths are placeholders under `flows/messaging/<adapter>/`; defaults can be shipped with packs, custom paths allow overrides.
- Telegram: keep two adapter entries (ingress + egress) for now, mirroring existing separate apps; a unified ingress-egress adapter can be added later if needed.
- Migration direction: adapters should be discovered via packs and invoked via greentic-runner using greentic-interfaces host bindings. New adapter components can wrap or reuse logic from `gsm-provider-registry`, but greentic-messaging should stop depending on the registry directly over time.
- Secrets/env: migrate credentials to greentic-secrets messaging conventions (`messaging/{adapter}/{tenant}/...`); keep current env vars as legacy bootstrap inputs and prefer seeding secrets/config from them at startup.
- Default packs: this table is the canonical list of default messaging packs; startup config should support “install all defaults” or a selectable subset (e.g., `MESSAGING_INSTALL_ALL_DEFAULT_ADAPTER_PACKS` / `MESSAGING_DEFAULT_ADAPTER_PACKS`).
