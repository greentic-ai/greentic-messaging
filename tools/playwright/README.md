# Playwright Screenshot Helper

Utility script for capturing authenticated screenshots against real UI pages.
This tool powers the conformance workflow, which uploads everything under
`tools/playwright/output/` as GitHub Actions artifacts.

## Prerequisites

- Node.js 20+
- Playwright browsers (install with `npx playwright install --with-deps`)

## Usage

```bash
npm ci
node index.mjs --permalink https://example.com/view/123 \
  --email "$TEST_LOGIN_EMAIL" \
  --password "$TEST_LOGIN_PASSWORD" \
  --out output/example.png
```

- `--permalink` – URL to capture.
- `--email` / `--password` – credentials used when a login form is detected. If
  omitted, the script reads `TEST_LOGIN_EMAIL` and `TEST_LOGIN_PASSWORD`.
- `--out` – destination image (defaults to `output/<timestamp>.png`).

The script uses simple heuristics to submit classic email/password forms. Extend
`index.mjs` with provider-specific automation when additional steps are needed
(MFA, captchas, etc.).

## Working with CI

`make conformance-*` clears and repopulates the `output/` directory before each
run. If you want to keep local captures, move them elsewhere or disable the cleanup
step in your shell session.
