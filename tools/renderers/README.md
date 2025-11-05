# Adaptive Card Renderer

Renders Adaptive Card payloads to static PNGs using Playwright and the
`adaptivecards` reference renderer. The conformance workflow copies the generated
images to GitHub Actions artifacts so stakeholders can review card diffs.

## Prerequisites

- Node.js 20+
- Playwright browsers (`npx playwright install --with-deps`)

## Usage

```bash
npm ci
node render.js --in ../../libs/cards/samples/weather.json --out output/weather.png
```

- `--in` – path to a JSON or YAML Adaptive Card document.
- `--out` – target PNG path. The directory is created on demand.

By default, the renderer launches headless Chromium, injects the card payload,
applies the stock host config, and captures the body. Tweak `render.js` if you
need branding overrides (fonts, colors, locale).

## CI integration

When `make conformance-*` runs, any files under `output/` are bundled as
`playwright-<provider>` artifacts. Keep only the captures you want to export
before invoking the suite.
