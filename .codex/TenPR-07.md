# TenPR-07 — Canonical config, secrets, and state handling

GOAL  
Ensure messaging never reimplements platform services.

TASKS

1) Config:
   - All config via greentic-config
   - Layering: defaults → file → env → CLI

2) Secrets:
   - All secrets via greentic-secrets
   - No direct env::var access for secrets

3) OAuth:
   - All OAuth flows via greentic-oauth

4) State:
   - State keyed by:
     env / tenant / team / user / conversation
   - No process-global state

5) Remove:
   - ad-hoc dotenv loading
   - legacy config structs
   - legacy secrets code

ACCEPTANCE
- Ripgrep for env::var(secret) returns nothing
- greentic-messaging is a clean consumer of greentic-* crates
