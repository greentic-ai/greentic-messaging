## Web Chat Demo

This Vite + React application exercises the Direct Line endpoints exposed by
`gsm_core::platforms::webchat` (PR-WC1 – PR-WC7).

### Development

```bash
cd examples/webchat-demo
npm install
npm run dev
```

The Vite dev server listens on <http://localhost:5174> and proxies every
`/v3/directline/**` request to `http://localhost:8090`. Start the standalone
Direct Line server with:

```bash
cargo run --manifest-path libs/core/Cargo.toml --example run_standalone
```

Optional overrides live in `examples/webchat-demo/.env.local`
or your repository `.env`. Supply either `WEBCHAT_*` or `VITE_WEBCHAT_*` keys (both
prefixes are recognised):

```ini
WEBCHAT_ENV=dev
WEBCHAT_TENANT=acme
WEBCHAT_TEAM=support                             # optional
WEBCHAT_BASE_URL=https://messaging.example.com   # optional, used for fetches
WEBCHAT_DIRECTLINE_DOMAIN=https://localhost:8080/v3/directline  # optional override
WEBCHAT_USER_ID=greentic-demo-user               # optional
```

The demo:

1. Calls `POST /v3/directline/tokens/generate?env=...&tenant=...`.
2. Starts a conversation via `POST /v3/directline/conversations` with the issued token.
3. Connects the Bot Framework Web Chat control to the returned conversation token with `domain: '/v3/directline'` (relative so the Vite proxy keeps requests same-origin) and `webSocket: false` for a simple polling loop. Specify `WEBCHAT_DIRECTLINE_DOMAIN` only when you need a different host.
4. Shows Adaptive Card submissions and other events echoing through the provider.

The UI also includes a “New conversation” button to refresh the Direct Line session without reloading the page.

### Notes

- When upgrading dependencies you may still see `"defaultProps will be removed"` warnings that originate from BotFramework Web Chat. They are upstream-only and safe to ignore in dev builds.
- A basic `favicon.ico` ships in `public/` to stop 404 noise in your console; replace it with your branding as needed.
- The Vite config polyfills common Node built-ins (`path`, `stream`, `zlib`, etc.) and pins `globalThis`/`process.env` so browser bundles stop “Module externalized” warnings. If you ever see them again, search for server-only imports leaking into `src/`.
- `source-map`/`source-map-js` is shimm’d via `src/shims/source-map-js.ts` because some debugging helpers inside BotFramework Web Chat try to pull Node’s stack-trace helpers into the browser bundle. If you later isolate those helpers to server-only code you can delete the shim and the matching `resolve.alias` entries.
- All Direct Line fetches log to the browser console while `npm run dev` is running. Look for `[webchat-demo] POST ...` entries to confirm requests reached `/v3/directline/**`, and inspect the paired response logs when you need to debug CORS/token issues.
