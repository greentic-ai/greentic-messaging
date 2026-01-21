# TenPR-05 — Single-tenant dev CLI

GOAL  
Provide a zero-friction local dev experience.

COMMANDS
- messaging dev up
- messaging dev logs
- messaging dev setup <provider>

DEFAULTS
- --tunnel cloudflared (ON)
- --subscriptions (ON, no-op if unsupported)
- PUBLIC_BASE_URL auto-detected & exported
- --packs-root ./packs

TASKS

1) Implement `messaging dev up`
   - Start gateway, runner, egress
   - Spawn cloudflared by default
   - Capture public URL
   - Set PUBLIC_BASE_URL for all child processes
   - Write runtime info to `.greentic/dev/`

2) Implement `messaging dev logs`
   - Tail logs from all components
   - Prefix logs with component name

3) Implement `messaging dev setup <provider>`
   - Use ProviderExtensionsRegistry
   - Use greentic-config, greentic-secrets, greentic-oauth
   - No ad-hoc HTTP or env parsing

ACCEPTANCE
- `messaging dev up` works on a laptop with no flags
- Public webhook URL is printed automatically
