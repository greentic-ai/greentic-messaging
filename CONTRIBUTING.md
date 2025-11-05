# Contributing to Greentic Messaging

Thanks for helping us keep the multi-provider messaging stack healthy. This guide
covers the local tooling you need, how to run offline checks, and how to exercise
the contract/E2E suites that hit real provider APIs.

## Local prerequisites

- Rust (stable) via `rustup` – CI uses the stable channel.
- Node.js 20+ – required for Playwright screenshot tooling and Adaptive Card rendering.
- Docker Engine + Docker Compose v2 – used to spin up the local NATS/mocks stack.
- Make or Just (GNU Make is bundled on macOS/Linux; `just` is optional but handy).

> Tip: run `rustup component add rustfmt clippy` so `cargo fmt` and `cargo clippy`
> are available.

## Fast feedback loop

These commands run quickly and should stay green before you open a PR:

```bash
make fmt           # cargo fmt --all
make lint          # cargo clippy --all-targets -- -D warnings || true
make test          # cargo test (fast, offline)
```

The default `ci` workflow executes the same steps on every push and pull request.

## Contract and E2E tests

The contract suite exercises live Slack/Telegram/Webex/WhatsApp/Teams APIs. Each
provider test auto-skips when secrets are missing, so you can opt-in one at a time.
Expect the setup to take ~5 minutes per provider once you have the credentials handy.

1. **Start the local stack**

   ```bash
   make stack-up        # launches NATS + mock webhooks (docker/stack.yml)
   ```

   Tear it down afterwards with `make stack-down`.

2. **Export provider secrets**

   See `providers/<platform>/README.md` for the exact tokens/IDs required. The
   readmes include quick-start links and helper scripts (for example,
   `scripts/telegram_setup.rs` resolves chat IDs).

   Example `.env` snippet:

   ```bash
   # Slack
   export SLACK_BOT_TOKEN=xoxb-...
   export SLACK_CHANNEL_ID=C12345678
   ```

3. **Run the provider suite**

   ```bash
   make conformance-slack         # or telegram/webex/whatsapp/teams
   make conformance               # runs every configured provider
   ```

   - Test output goes to the console.
   - Screenshots and HTTP payload samples land under `tools/playwright/output/`
     and `target/e2e-artifacts/`. They are collected automatically by CI.

4. **Cleanup**

   ```bash
   make stack-down
   ```

## Continuous integration

- `.github/workflows/ci.yml` runs on every push/PR and executes the fast offline checks.
- `.github/workflows/conformance.yml` runs nightly (UTC 04:00) and on manual dispatch.
  Each matrix job validates that its secrets exist before running `make
  conformance-<provider>`. Missing secrets produce a GitHub Actions notice rather
  than a failure.

If you manage org secrets, populate the tokens documented by each provider so the
nightly jobs exercise real environments.

## Troubleshooting quick hits

- `npx playwright install --with-deps` must run once per machine (the CI workflow
  does this automatically).
- Telegram chat IDs can be resolved with
  `cargo run --manifest-path scripts/Cargo.toml --bin telegram_setup -- --handle <@handle> --token <bot_token>`.
- Teams chat IDs and service principals can be verified with
  `cargo run --manifest-path scripts/Cargo.toml --bin teams_setup`.
- WhatsApp tests expect a Business API phone number; verify that the account can
  send interactive message templates before running the suite.

## Submitting a pull request

- Keep PRs scoped to a single milestone ticket (PR-XX).
- Run `make fmt`, `make lint`, and `make test`.
- Include contract tests when you touch provider integrations (see provider readmes
  for the required secrets).
- Add tests for new logic and update docs when behaviour changes.
