# GM-PR-01 — Messaging conformance runner (requirements → setup → ingress → egress → subscriptions)

REPO: greentic-ai/greentic-messaging

IMPORTANT CONTEXT
- Provider packs already exist and are validated structurally.
- We now need to prove they are FUNCTIONAL.
- This PR must reuse:
  - existing gateway / runner / egress code paths
  - greentic-provision for setup
- Do NOT create a parallel messaging runtime.

GOAL
Add a **messaging conformance runner** that executes the full provider lifecycle
in **dry-run mode by default**, with optional live mode behind explicit flags.

---

## DELIVERABLES

### 1) New CLI subcommand
Add (or extend existing test CLI):

`greentic-messaging-test e2e`

Flags:
- `--packs <dir>` (required): directory containing `messaging-*.gtpack`
- `--provider <name>` (optional filter)
- `--report <path>` (optional JSON output)
- `--dry-run` (default true)
- `--live` (requires RUN_LIVE_TESTS=true and RUN_LIVE_HTTP=true)
- `--trace`

---

### 2) Pack discovery & filtering
- Enumerate `messaging-*.gtpack` in `--packs`
- Load via existing pack loader
- Fail early if:
  - pack cannot be loaded
  - no messaging provider extensions found

---

### 3) Conformance stages (reuse real code)

#### Stage A — Requirements
For each pack:
- If `requirements` entry flow exists:
  - execute it via existing flow execution path (dry-run)
- Else:
  - infer requirements from:
    - config schemas
    - secret requirements asset

Record:
- required config keys
- required secret keys
- oauth required (Y/N)
- subscriptions required (Y/N)

Fail if requirements execution traps or returns invalid structure.

---

#### Stage B — Setup / Provisioning
- Invoke **greentic-provision** in dry-run:
  - tenant ctx (single-tenant dev ok)
  - provider id + install id
  - `public_base_url` from fixture
  - answers from `fixtures/setup.input.json`
- Capture `ProvisionPlan`

Fail if:
- setup traps
- output is nondeterministic
- required outputs (webhook/subscription ops) missing

---

#### Stage C — Ingress
If provider declares ingress:
- Load `fixtures/ingress.request.json`
- Route through existing gateway ingress path
- Assert:
  - structured ChannelMessage produced
  - required metadata present (tenant, provider, channel)
  - no panic/trap

---

#### Stage D — Runner flow
- Feed ChannelMessage into runner
- Execute minimal/default flow (or stub flow if provider pack doesn’t include one)
- Assert:
  - execution completes
  - state transitions are deterministic
  - no panic/trap

---

#### Stage E — Egress
If provider declares egress:
- Load `fixtures/egress.request.json`
- Route through existing egress path
- In dry-run:
  - produce request summary/plan
- In live:
  - execute send (gated)

Assert:
- structured output
- no panic/trap

---

#### Stage F — Subscriptions (if applicable)
If provider declares subscriptions:
- Execute `sync-subscriptions` via greentic-provision dry-run
- Assert:
  - operations are well-formed
  - deterministic output

---

### 4) Determinism & sandboxing
- Disable network by default
- Disable filesystem writes
- Fix random seed
- Enforce per-stage timeout

Failures must return diagnostics, not panics.

---

### 5) Report format
Produce JSON report:

```json
{
  "pack": "messaging-telegram",
  "version": "0.4.15",
  "status": "pass|fail",
  "stages": {
    "requirements": "pass",
    "setup": "pass",
    "ingress": "pass",
    "runner": "pass",
    "egress": "pass",
    "subscriptions": "skip|pass|fail"
  },
  "diagnostics": [...],
  "timing_ms": { "total": 234 }
}
Aggregate summary at top.

6) Tests
Add tests/e2e/:

minimal dummy messaging pack

assert full dry-run passes

ACCEPTANCE CRITERIA
greentic-messaging-test e2e --dry-run runs across all messaging packs

Failures are localized to pack + stage

No network calls by default

yaml
Copy code

---

## **GM-PR-02.md — Provider installation records + runtime routing**

```md
# GM-PR-02 — Provider installation records + runtime routing

REPO: greentic-ai/greentic-messaging

IMPORTANT CONTEXT
- Provider setup produces outputs (config/secrets/webhooks/subscriptions).
- Runtime must consume those outputs consistently.
- No env::var reads in runtime.

GOAL
Make messaging runtime driven entirely by **provider installation records**
created by greentic-provision.

---

## DELIVERABLES

### 1) Installation record integration
- Integrate `ProviderInstallRecord` model (from greentic-types / greentic-provision)
- Store records per:
  - tenant
  - provider_id
  - install_id

Use in-memory store for dev/tests; pluggable later.

---

### 2) Gateway routing
For inbound messages:
- resolve provider install via:
  - provider_id
  - channel/platform identifiers
- load config/secrets from install record
- validate webhook signatures using install record state

---

### 3) Egress routing
For outbound messages:
- select provider install explicitly (provider_id + install_id)
- use config/secrets from install record
- support multiple installs of same provider

---

### 4) Failure handling
- Missing install → structured error
- Missing secret → structured error
- Single-provider failure must not crash runtime

---

### 5) Tests
- two installs of same provider route correctly
- missing install produces diagnostic
- no env::var usage in runtime path

---

## ACCEPTANCE CRITERIA
- Messaging runtime works with multiple provider installs
- All config/secrets sourced from install records
GM-PR-03.md — Subscriptions worker integration
md
Copy code
# GM-PR-03 — Subscriptions worker integration for messaging providers

REPO: greentic-ai/greentic-messaging

IMPORTANT CONTEXT
- Some providers (Teams, etc.) require long-lived subscriptions.
- Subscriptions are declared and planned by provider packs.
- greentic-provision produces subscription ops.

GOAL
Add a **subscriptions worker** that keeps provider subscriptions alive
using installation records.

---

## DELIVERABLES

### 1) Subscriptions discovery
- On startup:
  - list provider installs with subscriptions capability
- Register them with the worker

---

### 2) Execution loop
- Periodically run:
  - `sync-subscriptions` (or lifecycle ops) via greentic-provision
- Apply resulting updates to install record:
  - subscription id
  - expiry
  - last_sync

---

### 3) Failure handling
- Transient failures → retry with backoff
- Repeated failures → mark install as degraded
- Never crash the service due to one provider

---

### 4) Tests
- dummy provider with subscriptions sync
- ensure next expiry recorded
- failure does not crash worker

---

## ACCEPTANCE CRITERIA
- Teams-style providers keep subscriptions alive
- Subscriptions state persists in install record
GM-PR-04.md — Dev UX: cloudflared default + setup + logs
md
Copy code
# GM-PR-04 — Dev UX: cloudflared by default + dev setup + dev logs

REPO: greentic-ai/greentic-messaging

GOAL
Make local, single-tenant provider testing trivial.

---

## DELIVERABLES

### 1) `messaging dev up`
- Starts:
  - gateway
  - runner
  - egress
- Starts cloudflared tunnel by default
- Auto-detects `PUBLIC_BASE_URL`
- Persists it for setup use

---

### 2) `messaging dev setup <provider>`
- Runs greentic-provision setup for provider
- Uses stored `PUBLIC_BASE_URL`
- Stores provider installation record
- Supports `--update` and `--delete`

---

### 3) `messaging dev logs`
- Streams logs from all services
- Prefix logs with component name
- Supports `--follow`

---

### 4) Tests
- dev up + setup dummy provider (dry-run)
- verify install record created

---

## ACCEPTANCE CRITERIA
- A laptop can do:
  - `messaging dev up`
  - `messaging dev setup telegram`
  - receive ingress locally
GM-PR-05.md — Runtime config/secrets discipline (TenPR-07 enforcement)
md
Copy code
# GM-PR-05 — Enforce config/secrets discipline in messaging runtime (TenPR-07)

REPO: greentic-ai/greentic-messaging

IMPORTANT CONTEXT
- Runtime must not read secrets/config from env vars.
- Tooling/tests may still use env for bootstrap.

GOAL
Eliminate env::var usage for config/secrets in runtime services.

---

## DELIVERABLES

### 1) Scope enforcement
IN SCOPE:
- apps/messaging-gateway
- apps/runner
- apps/messaging-egress
- libs used by these binaries

OUT OF SCOPE:
- tools/*
- tests/*
- examples/*
- dev CLIs (bootstrap only)

---

### 2) Refactor runtime code
- Replace env::var usage with:
  - greentic-config
  - greentic-secrets
- Ensure secrets never flow via env in runtime path

---

### 3) Guardrails
- Add lint/test:
  - ripgrep for `env::var` in runtime crates
- Fail CI if found

---

### 4) Tests
- runtime starts without env secrets
- missing config/secrets produce structured diagnostics

---

## ACCEPTANCE CRITERIA
- No env::var usage for config/secrets in runtime
- CI blocks regressions