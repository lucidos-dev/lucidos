import { defineConfig } from '@playwright/test';
import { readAddress } from './e2e/address';
import { DESKTOP_IGNORES, DESKTOP_WEBKIT_SPECS, PHONE_SPECS } from './e2e/projectRules';

const { port, proto } = readAddress();

const WEBKIT_HARNESS = {
  proxy: { server: 'http://127.0.0.1:1', bypass: 'localhost,127.0.0.1,::1' },
  serviceWorkers: 'block',
} as const;

export default defineConfig({
  testDir: './e2e',
  testMatch: '**/*.spec.ts',
  timeout: 120_000,
  expect: { timeout: 30_000 },
  fullyParallel: false,
  retries: 1,
  workers: 1,
  reporter: [['list'], ['./e2e/memSampleReporter.ts']],
  use: {
    baseURL: `${proto}://localhost:${port}`,
    ignoreHTTPSErrors: true,
    headless: !process.env.HEADED,
    viewport: { width: 1280, height: 800 },
    actionTimeout: 15_000,
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
    reducedMotion: 'reduce',
  },
  projects: [
    {
      name: 'chromium',
      testIgnore: DESKTOP_IGNORES,
      use: { browserName: 'chromium' },
    },
    {
      name: 'mobile',
      testMatch: PHONE_SPECS,
      use: {
        browserName: 'chromium',
        viewport: { width: 375, height: 812 },
        isMobile: true,
        hasTouch: true,
      },
    },
    {
      name: 'desktop-webkit',
      testMatch: DESKTOP_WEBKIT_SPECS.map(spec => `**/${spec}`),
      use: { browserName: 'webkit', ...WEBKIT_HARNESS },
    },
    {
      name: 'mobile-webkit',
      testMatch: PHONE_SPECS,
      use: {
        browserName: 'webkit',
        viewport: { width: 390, height: 844 },
        isMobile: true,
        hasTouch: true,
        deviceScaleFactor: 3,
        userAgent: 'Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1',
        ...WEBKIT_HARNESS,
      },
    },
  ],
});
