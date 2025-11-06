## Web Chat Demo

This Vite + React application exercises the Direct Line endpoints exposed by
`gsm_core::platforms::webchat` (PR-WC1 – PR-WC7).

### Development

```bash
cd examples/webchat-demo
npm install
npm run dev
```

By default the dev server proxies `/v3/directline` to `http://localhost:8090`.
Start the standalone server with
`cargo run --manifest-path libs/core/Cargo.toml --example run_standalone` and no
other setup is required. Optional overrides live in `examples/webchat-demo/.env.local`
or your repository `.env`. Supply either `WEBCHAT_*` or `VITE_WEBCHAT_*` keys (both
prefixes are recognised):

```ini
WEBCHAT_ENV=dev
WEBCHAT_TENANT=acme
WEBCHAT_TEAM=support                             # optional
WEBCHAT_BASE_URL=https://messaging.example.com   # optional
WEBCHAT_DIRECTLINE_DOMAIN=https://localhost:8080/v3/directline  # optional
WEBCHAT_USER_ID=greentic-demo-user               # optional
```

The demo:

1. Calls `POST /v3/directline/tokens/generate?env=...&tenant=...`.
2. Starts a conversation via `POST /v3/directline/conversations` with the issued token.
3. Connects the Bot Framework Web Chat control to the returned conversation token and `domain` pointing at the standalone Direct Line endpoint (defaults to `https://localhost:8080/v3/directline`).
4. Shows Adaptive Card submissions and other events echoing through the provider.

The UI also includes a “New conversation” button to refresh the Direct Line session without reloading the page.
