import fs from 'fs/promises';
import path from 'path';
import { chromium } from '@playwright/test';

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

async function tryLogin(page, email, password) {
  // Selector hints only; no credentials are embedded in the codebase.
  const attempts = [
    {
      emailSelector: 'input[name="email"]',
      passwordSelector: 'input[name="password"]',
    },
    {
      emailSelector: 'input[type="email"]',
      passwordSelector: 'input[type="password"]',
    },
  ];

  for (const attempt of attempts) {
    try {
      const emailField = await page.$(attempt.emailSelector);
      const passwordField = await page.$(attempt.passwordSelector);
      if (!emailField || !passwordField) {
        continue;
      }

      await emailField.fill(email, { timeout: 2000 });
      await passwordField.fill(password, { timeout: 2000 });

      const submitSelector = 'button[type="submit"],input[type="submit"]';
      const submit = await page.$(submitSelector);
      if (submit) {
        await Promise.all([
          submit.click({ timeout: 2000 }),
          page.waitForLoadState('networkidle', { timeout: 10000 }).catch(() => null),
        ]);
      } else {
        await passwordField.press('Enter');
        await page.waitForLoadState('networkidle', { timeout: 10000 }).catch(() => null);
      }
      return true;
    } catch (error) {
      // Try the next selector set.
      console.warn('Playwright login attempt failed:', error.message);
    }
  }
  return false;
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const permalink = args.permalink;
  if (!permalink) {
    console.error('Missing required argument: --permalink <url>');
    process.exit(1);
  }

  const email = args.email ?? process.env.TEST_LOGIN_EMAIL ?? '';
  const password = args.password ?? process.env.TEST_LOGIN_PASSWORD ?? '';
  const output = path.resolve(
    args.out ?? path.join('output', `screenshot-${Date.now()}.png`),
  );

  await fs.mkdir(path.dirname(output), { recursive: true });

  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext();
  const page = await context.newPage();

  await page.goto(permalink, { waitUntil: 'load' });

  if (email && password) {
    const loggedIn = await tryLogin(page, email, password);
    if (loggedIn) {
      await page.goto(permalink, { waitUntil: 'networkidle' });
    }
  } else {
    await page.waitForLoadState('networkidle').catch(() => null);
  }

  await page.waitForTimeout(1000);
  await page.screenshot({ path: output, fullPage: true });
  console.log(`Saved screenshot to ${output}`);

  await browser.close();
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
