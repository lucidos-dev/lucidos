import { defineConfig } from '@playwright/test';
// The workspace records both halves of its own address, and e2e/address.ts is
// the one place that reads them. See its header for why the protocol half is
// not optional.
import { readAddress } from './e2e/address';
import { DESKTOP_IGNORES, PHONE_SPECS } from './e2e/projectRules';

const { port, proto } = readAddress();

export default defineConfig({
  testDir: './e2e',
  // Only specs. The default also takes `*.test.ts`, which here are Vitest unit
  // tests for the harness itself.
  testMatch: '**/*.spec.ts',
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
  reporter: [['list'], ['./e2e/memSampleReporter.ts']],
  use: {
    baseURL: `${proto}://localhost:${port}`,
    ignoreHTTPSErrors: true,
    headless: !process.env.HEADED,
    viewport: { width: 1280, height: 800 },
    actionTimeout: 15_000,
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
    // Calm motion by default. The Motion setting follows the OS, so every
    // glide, fade and FLIP jumps to its end, and no spec waits one out. A spec
    // that tests motion opts back in with `reducedMotion: 'no-preference'`
    // (e2e/motion-stance.test.ts, docs/e2e-test-decisions.md § Calm motion).
    reducedMotion: 'reduce',
  },
  // One browser by default: the suffix picks the projects (e2e/projectRules.ts).
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
      name: 'mobile-webkit',
      testMatch: PHONE_SPECS,
      use: {
        browserName: 'webkit',
        viewport: { width: 390, height: 844 },
        isMobile: true,
        hasTouch: true,
        deviceScaleFactor: 3,
        userAgent: 'Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1',
        // ROOT-CAUSE FIX for the "WebContent nav-wedge" (first page.goto in a
        // fresh context times out, WebKit-only — see docs/e2e-test-decisions.md
        // "mobile-webkit navigation wedge"). On a managed/MDM Mac the macOS
        // system network config can carry proxy *auto-discovery* (WPAD) or a PAC
        // URL. Playwright's WebKit network process honors the system proxy by
        // default, so the FIRST navigation in each fresh context (= fresh network
        // session) synchronously runs WPAD/PAC discovery before it issues the
        // request — a DNS/captive-portal/PAC-fetch round trip that stalls for
        // tens of seconds under load, then self-clears (which is why a fresh
        // context recovers and why the engine never sees the `/` request). It is
        // WebKit-only because Playwright's Chromium is launched without the
        // system proxy. Setting an EXPLICIT proxy here makes WebKit skip system
        // auto-discovery entirely; `bypass` routes our (localhost-only) e2e
        // traffic DIRECT so the proxy is never actually contacted. `server` is a
        // deliberately-inert loopback dead port — present only to disable
        // discovery, fast-refused if ever hit (it won't be: every URL the suite
        // loads is localhost). This prevents the wedge at the source rather than
        // recovering from it via retry.
        proxy: { server: 'http://127.0.0.1:1', bypass: 'localhost,127.0.0.1,::1' },
      },
    },
  ],
});
