import { defineConfig } from '@playwright/test';
import { readFileSync, existsSync } from 'fs';
import { resolve } from 'path';

const WORKSPACE = resolve(process.env.E2E_WORKSPACE ?? `${process.env.HOME}/workspaces/e2e-test`);
const portsFile = resolve(WORKSPACE, '.lucidos/ports');

function readPort(): number {
  if (!existsSync(portsFile)) {
    throw new Error(`Ports file not found: ${portsFile}. Start the workspace first: ./scripts/web-dev.sh -w ${WORKSPACE} -b`);
  }
  const content = readFileSync(portsFile, 'utf-8');
  const match = content.match(/VITE_PORT=(\d+)/);
  if (!match) throw new Error(`VITE_PORT not found in ${portsFile}`);
  return parseInt(match[1], 10);
}

const port = readPort();

export default defineConfig({
  testDir: './e2e',
  timeout: 120_000,
  expect: { timeout: 30_000 },
  fullyParallel: false,
  // One automatic retry to absorb rare browser-launch / context-init flakes
  // (e.g. "browser.newContext: Target page, context or browser has been
  // closed" mid-suite — Playwright/Chromium occasionally crashes between
  // tests under load). A real test or app bug fails twice and still surfaces;
  // a true flake passes on the retry. Reduces signal loss from infra noise.
  retries: 1,
  workers: 1,
  reporter: 'list',
  use: {
    baseURL: `https://localhost:${port}`,
    ignoreHTTPSErrors: true,
    headless: !process.env.HEADED,
    viewport: { width: 1280, height: 800 },
    actionTimeout: 15_000,
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
  },
  projects: [
    {
      name: 'chromium',
      // *-mobile.spec.ts files target mobile-only UI (edge-swipe overlay,
      // touch-specific stacking contexts) that doesn't render on desktop —
      // exclude them rather than runtime-skipping so the chromium results
      // stay clean.
      testIgnore: /-mobile\.spec\.ts$/,
      use: { browserName: 'chromium' },
    },
    {
      name: 'mobile',
      use: {
        browserName: 'chromium',
        viewport: { width: 375, height: 812 },
        isMobile: true,
        hasTouch: true,
      },
    },
    {
      name: 'mobile-webkit',
      use: {
        browserName: 'webkit',
        viewport: { width: 390, height: 844 },
        isMobile: true,
        hasTouch: true,
        deviceScaleFactor: 3,
        userAgent: 'Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1',
      },
    },
  ],
});
