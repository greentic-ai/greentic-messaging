# Messaging adapters inventory (pack-based)

Adapters are now discovered from messaging packs (.gtpack preferred, YAML only for development).
Load them via `MESSAGING_ADAPTER_PACK_PATHS`/`greentic-messaging --pack …`. The legacy
`gsm-provider-registry` implementations are being retired once pack-backed components are
validated; keep them only as a fallback during migration.

| adapter_name (pack entry) | kind | pack file | component id | flow references | notes |
| --- | --- | --- | --- | --- | --- |
| `slack-main` | ingress-egress | `packs/messaging/slack.yaml` / `target/packs/greentic-messaging-slack.gtpack` | `slack-adapter@1.0.0` | `flows/messaging/slack/default.ygtc`, `flows/messaging/slack/custom.ygtc` | Requires `messaging/slack.credentials.json` secret. |
| `teams-main` | ingress-egress | `packs/messaging/teams.yaml` / `target/packs/greentic-messaging-teams.gtpack` | `teams-adapter@1.0.0` | `flows/messaging/teams/default.ygtc`, `flows/messaging/teams/custom.ygtc` | Requires `messaging/teams.credentials.json` secret. |
| `webex-main` | ingress-egress | `packs/messaging/webex.yaml` / `target/packs/greentic-messaging-webex.gtpack` | `webex-adapter@1.0.0` | `flows/messaging/webex/default.ygtc`, `flows/messaging/webex/custom.ygtc` | Requires `messaging/webex.credentials.json` secret. |
| `webchat-main` | ingress-egress | `packs/messaging/webchat.yaml` / `target/packs/greentic-messaging-webchat.gtpack` | `webchat-adapter@1.0.0` | `flows/messaging/webchat/default.ygtc`, `flows/messaging/webchat/custom.ygtc` | Requires `messaging/webchat.credentials.json` secret. |
| `whatsapp-main` | ingress-egress | `packs/messaging/whatsapp.yaml` / `target/packs/greentic-messaging-whatsapp.gtpack` | `whatsapp-adapter@1.0.0` | `flows/messaging/whatsapp/default.ygtc`, `flows/messaging/whatsapp/custom.ygtc` | Requires `messaging/whatsapp.credentials.json` secret. |
| `telegram-ingress` | ingress | `packs/messaging/telegram.yaml` / `target/packs/greentic-messaging-telegram.gtpack` | `telegram-ingress-adapter@1.0.0` | `flows/messaging/telegram/ingress_default.ygtc`, `flows/messaging/telegram/ingress_custom.ygtc` | Requires `messaging/telegram.credentials.json` secret. |
| `telegram-egress` | egress | `packs/messaging/telegram.yaml` / `target/packs/greentic-messaging-telegram.gtpack` | `telegram-egress-adapter@1.0.0` | `flows/messaging/telegram/egress_default.ygtc`, `flows/messaging/telegram/egress_custom.ygtc` | Requires `messaging/telegram.credentials.json` secret. |
| `local-main` | ingress-egress | `packs/messaging/local.yaml` / `target/packs/greentic-messaging-local.gtpack` | `local-adapter@1.0.0` | `flows/messaging/local/default.ygtc`, `flows/messaging/local/custom.ygtc` | Dev/mock adapter, no external deps. |

Legacy provider-registry code paths (`libs/gsm-provider-registry/**`, `apps/ingress-*`, `apps/egress-*`)
will be dropped once all gtpack components are validated with greentic-runner. Prefer testing and
deploying via packs so platform secrets, components, and flows remain declarative.
