# Messaging adapter loading and selection

This repo ships default messaging adapter packs and loads them at startup. Adapter resolution is now registry-driven and pack-based.

Environment flags:
- `MESSAGING_INSTALL_ALL_DEFAULT_ADAPTER_PACKS=true|false` (default false): load all packs under `packs/messaging/`.
- `MESSAGING_DEFAULT_ADAPTER_PACKS=teams,slack,...`: explicit subset of default packs (ignored if `install_all` is true).
- `MESSAGING_ADAPTER_PACK_PATHS=/abs/path/a.yaml,/abs/path/b.yaml`: extra pack files to load.
- `MESSAGING_PACKS_ROOT=...`: root directory that contains `messaging/` (defaults to `packs`).
- `MESSAGING_EGRESS_ADAPTER=name`: force a specific egress adapter; otherwise selection is by platform.

Ingress (gateway):
- If a registry exists and a `:channel` matches an adapter name, it must support ingress or a 400 is returned.
- If a registry exists but the name is missing, a 400 lists available adapters.
- Adapter name is added to message metadata; NATS subject uses the resolved platform.

Egress:
- Adapter is resolved by override (`MESSAGING_EGRESS_ADAPTER`) or by platform prefix match on adapter name.
- Egress logs and metrics include the adapter/component/flow (if provided in the pack).
- Runner: `MESSAGING_RUNNER_HTTP_URL` / `MESSAGING_RUNNER_HTTP_API_KEY` configure the HTTP RunnerClient used by messaging-egress; defaults to a logging client if unset.

Metrics:
- Gateway: `messaging_ingress_total{tenant,platform,adapter}` increments on publish.
- Egress: `messaging_egress_total{tenant,platform,adapter}` increments on processing; runner invocations emit `messaging_egress_runner_success_total` / `messaging_egress_runner_failure_total`.

Next steps:
- Route secrets via greentic-secrets conventions; emit deprecation warnings for legacy env-based adapters.
