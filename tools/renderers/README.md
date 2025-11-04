# Adaptive Card Renderer

Renders Adaptive Card payloads to static PNGs using Playwright and the
`adaptivecards` reference renderer. Requires Node.js 18+.

## Usage

```bash
npm ci
node render.js --in ../../libs/cards/samples/weather.json --out output/weather.png
```

- `--in` – path to a JSON or YAML Adaptive Card document.
- `--out` – target PNG path. The directory is created when needed.

By default, the renderer launches a headless Chromium instance, injects the card
payload, and captures the resulting DOM. You can customise the host config or
styling in `render.js` to match upstream branding.
