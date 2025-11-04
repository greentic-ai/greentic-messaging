# Playwright Screenshot Helper

Utility script for capturing authenticated screenshots against real UI pages. The
tool expects Node.js 18+ and the Playwright runtime (installed through
`npm ci`).

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
  they are omitted the script falls back to `TEST_LOGIN_EMAIL` and
  `TEST_LOGIN_PASSWORD` environment variables.
- `--out` – target screenshot path; defaults to the `output/` directory.

The script uses a very small heuristic to submit traditional email/password
forms. Complex providers can enhance the flow by adding custom automation to
`index.mjs`.

> Tip: run `npx playwright install` once locally so the required browser
> binaries are available offline.
