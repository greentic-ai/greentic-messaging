import fs from 'fs/promises';
import path from 'path';
import { createRequire } from 'module';
import { chromium } from '@playwright/test';

const require = createRequire(import.meta.url);

function parseArgs(argv) {
  const args = {};
  for (let i = 0; i < argv.length; i += 1) {
    const current = argv[i];
    if (!current.startsWith('--')) {
      continue;
    }
    const key = current.slice(2);
    const value = argv[i + 1];
    if (value && !value.startsWith('--')) {
      args[key] = value;
      i += 1;
    } else {
      args[key] = true;
    }
  }
  return args;
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const inputPath = args.in;
  const outputPath = path.resolve(args.out ?? 'output/card.png');

  if (!inputPath) {
    console.error('Missing required argument: --in <card.json>');
    process.exit(1);
  }

  const raw = await fs.readFile(inputPath, 'utf8');
  const payload = JSON.parse(raw);
  await fs.mkdir(path.dirname(outputPath), { recursive: true });

  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({ deviceScaleFactor: 2 });
  const page = await context.newPage();

  const adaptivePath = require.resolve('adaptivecards/dist/adaptivecards.js');

  const html = `<!doctype html>
  <html>
    <head>
      <meta charset="utf-8" />
      <style>
        body {
          margin: 24px;
          background: #f5f6fa;
          font-family: "Segoe UI", sans-serif;
        }
        #card-root {
          max-width: 480px;
        }
      </style>
    </head>
    <body>
      <div id="card-root"></div>
    </body>
  </html>`;

  await page.setContent(html, { waitUntil: 'load' });
  await page.addScriptTag({ path: adaptivePath });

  await page.evaluate((cardPayload) => {
    const card = new window.AdaptiveCards.AdaptiveCard();
    card.hostConfig = new window.AdaptiveCards.HostConfig({
      fontStyles: {
        default: {
          fontFamily: '"Segoe UI", sans-serif',
        },
      },
    });
    card.parse(cardPayload);
    const rendered = card.render();
    const root = document.getElementById('card-root');
    root.innerHTML = '';
    root.appendChild(rendered);
  }, payload);

  const bounds = await page.evaluate(() => ({
    width: document.documentElement.scrollWidth,
    height: document.documentElement.scrollHeight,
  }));

  const width = Math.max(320, Math.ceil(bounds.width));
  const height = Math.max(200, Math.ceil(bounds.height));
  await page.setViewportSize({ width, height });
  await page.waitForTimeout(250);

  await page.screenshot({ path: outputPath, omitBackground: true });
  console.log(`Rendered Adaptive Card to ${outputPath}`);

  await browser.close();
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
