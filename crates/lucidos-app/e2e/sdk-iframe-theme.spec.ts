import { test, expect } from './fixtures';
import { appPath, createIframeAppFixture, psql } from './db-helpers';
import { gotoWithRetry } from './helpers';

// Verifies that an app iframe receives live theme updates via the SDK's
// `lucidos.ui.watchPreferences()` SSE subscription.
//
// The bug we're guarding against: PreferencesChanged events were broadcast
// engine-side but the iframe SDK either never connected its EventSource or
// never dispatched the event to the watchPreferences callback, so the iframe's
// data-theme stayed stale until reload.

const APP_ID = 'e2e-sdk-theme-test';
let fixture: { cleanup: () => void };

test.describe('SDK iframe theme — live PreferencesChanged update', () => {
  test.beforeAll(() => {
    // Minimal app: load SDK, apply current prefs, watch for changes.
    // No app code beyond the two SDK calls — this is the contract every app uses.
    fixture = createIframeAppFixture(APP_ID, {
      manifest: { id: APP_ID, name: 'SDK theme test', description: 'e2e fixture' },
      html: `<!DOCTYPE html>
<html>
<head>
<meta charset="UTF-8">
<title>SDK theme test</title>
<link rel="stylesheet" href="/api/v1/sdk-iframe.css">
<script src="/api/v1/sdk.js"></script>
</head>
<body>
<div id="status">init</div>
<script src="script.js"></script>
</body>
</html>
`,
      js: `
function waitForLucidos() {
  if (window.lucidos && window.lucidos.data) {
    lucidos.ui.applyPreferences().then(() => {
      document.getElementById('status').textContent = 'applied';
    });
    lucidos.ui.watchPreferences();
  } else {
    setTimeout(waitForLucidos, 100);
  }
}
waitForLucidos();
`,
    });
  });

  test.afterAll(async () => {
    fixture.cleanup();
    psql(`DELETE FROM preferences WHERE key = 'theme'`);
  });

  test('app inside Lucidos iframe receives live PreferencesChanged via SSE', async ({ page, request }) => {
    // Establish a known device id and seed initial theme (matches the parent's
    // localStorage; iframe shares same origin so it reads the same id).
    await gotoWithRetry(page, '/');
    const deviceId = 'e2e-device-' + Date.now();
    await page.evaluate((id) => localStorage.setItem('lucidos-device-id', id), deviceId);
    await request.put(`/api/v1/preferences?key=theme`, {
      data: { value: 'dark', device_id: deviceId },
    });

    // Embed the test app inside an iframe sandbox identical to AppUiInline.tsx.
    // The parent page also opens its own SSE listener (matches the real app
    // where Lucidos UI subscribes to the same /api/v1/events).
    await page.setContent(`<!DOCTYPE html>
<html>
<head><meta charset="UTF-8"></head>
<body>
<iframe
  id="app-frame"
  src="${appPath(APP_ID)}"
  sandbox="allow-scripts allow-same-origin allow-popups allow-forms allow-modals allow-popups-to-escape-sandbox"
  style="width:600px;height:400px;border:0"></iframe>
<script>
  // Mirror the parent Lucidos app — open an EventSource so connection-limit
  // contention with the iframe is realistic.
  window.__parentSse = new EventSource('/api/v1/events');
</script>
</body>
</html>`);

    const appFrame = page.frameLocator('#app-frame');
    await expect(appFrame.locator('#status')).toHaveText('applied');
    await expect(appFrame.locator('html')).toHaveAttribute('data-theme', 'dark');

    // Toggle to light — SSE event broadcast to all subscribers including the iframe.
    await request.put(`/api/v1/preferences?key=theme`, {
      data: { value: 'light', device_id: deviceId },
    });

    // Live update inside the iframe — should land within seconds.
    await expect(appFrame.locator('html')).toHaveAttribute('data-theme', 'light', { timeout: 5000 });
  });

  test('opt-in /api/v1/sdk-prefs.js serves a static localStorage-driven script', async ({ request }) => {
    // The endpoint no longer consults the device-id cookie or the DB — it
    // returns a script that reads localStorage at execution time. Iframes
    // share the parent's localStorage (same-origin, allow-same-origin), so
    // the script's first paint matches whatever the parent shell has stored.
    const res = await request.get('/api/v1/sdk-prefs.js');
    expect(res.status()).toBe(200);
    expect(res.headers()['content-type']).toContain('application/javascript');

    const js = await res.text();
    // IIFE prologue, with optional whitespace before the brace.
    expect(js).toMatch(/^\(function\(\)\s*\{/);
    // Storage keys are workspace-scoped via wsKey() (mirrors workspaceStorage.ts
    // + sdk/_storage.ts); the engine-side guard in sdk_prefs.rs forbids any raw,
    // unscoped access, so the served script always wraps the key in wsKey().
    expect(js).toContain('localStorage.getItem(wsKey("lucidos-theme"))');
    expect(js).toContain('localStorage.getItem(wsKey("lucidos-font-family"))');
    expect(js).toContain('localStorage.getItem(wsKey("lucidos-ui-scale"))');
    // `system` defers to matchMedia at execution time so light-OS browsers
    // don't FOUC dark-then-light.
    expect(js).toContain('matchMedia("(prefers-color-scheme: light)")');
    // Sets the parent-shell-shared CSS contract: data-theme + --bg-primary
    // + --font-ui (and --user-ui-scale when set).
    expect(js).toContain('setAttribute("data-theme"');
    expect(js).toContain('setProperty("--bg-primary"');
    expect(js).toContain('setProperty("--font-ui"');
  });
});

// Cold-load regression: simulates a returning user whose `lucidos-theme` is
// already persisted in localStorage. The parent shell's inline FOUC IIFE
// (in `crates/lucidos-app/index.html`) and the iframe's `/api/v1/sdk-prefs.js`
// must both read the same localStorage and paint `data-theme="light"` from
// frame zero — no flash to dark and back.
//
// localStorage seeding uses `addInitScript`, not `page.evaluate` after goto:
// the parent shell's inline IIFE reads localStorage during the very first
// `<head>` evaluation, so any post-goto seed is too late.

const COLD_APP_ID = 'e2e-sdk-cold-load';
let coldFixture: { cleanup: () => void };

test.describe('SDK iframe theme — cold reload (full Lucidos bootstrap)', () => {
  test.beforeAll(() => {
    coldFixture = createIframeAppFixture(COLD_APP_ID, {
      manifest: { id: COLD_APP_ID, name: 'SDK cold-load test', description: 'e2e fixture' },
      // Mirrors the standard app boilerplate: opt-in script first, then
      // shared stylesheet, then sdk.js. The body holds a `#ready` marker
      // and a script that calls `applyPreferences` + `watchPreferences` —
      // exactly the contract every real app follows.
      html: `<!DOCTYPE html>
<html>
<head>
<meta charset="UTF-8">
<title>Cold load test</title>
<script src="/api/v1/sdk-prefs.js"></script>
<link rel="stylesheet" href="/api/v1/sdk-iframe.css">
<script src="/api/v1/sdk.js"></script>
</head>
<body>
<div id="ready">ready</div>
<script src="script.js"></script>
</body>
</html>
`,
      js: `
function waitForLucidos() {
  if (window.lucidos && window.lucidos.ui) {
    lucidos.ui.applyPreferences();
    lucidos.ui.watchPreferences();
  } else {
    setTimeout(waitForLucidos, 50);
  }
}
waitForLucidos();
`,
    });
  });

  test.afterAll(() => {
    coldFixture.cleanup();
    psql(`DELETE FROM preferences WHERE key = 'theme'`);
  });

  test('parent and iframe paint data-theme="light" from frame zero — driven by localStorage', async ({ page, request, context }) => {
    const deviceId = 'e2e-cold-' + Date.now();

    // Seed the user's persisted state BEFORE the parent's first paint:
    //   - localStorage device id (so getDeviceId() picks it up; also drives
    //     the per-device API calls preferences.ts makes after mount)
    //   - localStorage lucidos-theme=light (this is what the inline FOUC IIFE
    //     in index.html reads on the very first <head> tick)
    //   - app-window-open (so loadApps() restores the test app on reload —
    //     mirrors the real "last app stays open across reloads" UX)
    await context.addInitScript(([id, appId]) => {
      localStorage.setItem('lucidos-device-id', id);
      localStorage.setItem('lucidos-theme', 'light');
      localStorage.setItem('app-window-open', appId);
    }, [deviceId, COLD_APP_ID]);

    // Persist theme=light for that device so the post-load `applyPreferences()`
    // SSE round-trip agrees with the localStorage cache. (Without this, the
    // backend would return its default and the iframe's SDK applyPreferences
    // call would later flip the theme back, masking a real FOUC.)
    await request.put(`/api/v1/preferences?key=theme`, {
      data: { value: 'light', device_id: deviceId },
    });

    // Cold reload — the auto-restore path mounts the iframe via
    // panelOverlay → AppUiInline. No `?app=` deep-link (its 500ms timeout
    // hides the bug behind a delay).
    await gotoWithRetry(page, '/');

    const iframeLoc = page.locator('iframe[data-role="app-ui-frame"]:visible');
    await expect(iframeLoc).toBeVisible({ timeout: 10_000 });

    // The iframe element's background follows the parent's --bg-primary,
    // which depends on the parent's data-theme. With localStorage-driven
    // FOUC, the parent's first paint is light from frame zero.
    const iframeBg = await page.evaluate(() =>
      getComputedStyle(document.querySelector('iframe[data-role="app-ui-frame"]')!).backgroundColor);
    expect(iframeBg, `iframe element bg must be white, not dark (got ${iframeBg})`).toMatch(/^rgba?\(\s*255\s*,\s*255\s*,\s*255/);

    // Pins the inline value separately from the iframeBg check above:
    // by query time CSS modules have hydrated, so iframeBg can pass on
    // luck while the user still saw `var(--bg-primary, #07172e)` fall
    // back to dark for the first ~50–200ms.
    const inlinedBgPrimary = await page.evaluate(() =>
      document.documentElement.style.getPropertyValue('--bg-primary'));
    expect(
      inlinedBgPrimary,
      'parent shell must inline --bg-primary on <html> from localStorage so the body var() resolves before external CSS loads',
    ).toBe('#ffffff');

    const appFrame = page.frameLocator('iframe[data-role="app-ui-frame"]:visible');
    await expect(appFrame.locator('#ready')).toBeVisible({ timeout: 10_000 });

    // Capture every data-theme transition inside the iframe. A FOUC
    // manifests as `dark → light` even when the steady state is "light";
    // a single assertion on the post-load attribute can't see the brief
    // dark frame.
    const iframeTransitions = await appFrame.locator('html').evaluate((html) => {
      return new Promise<string[]>((resolve) => {
        const seen: string[] = [html.getAttribute('data-theme') ?? '<unset>'];
        const obs = new MutationObserver(() => {
          seen.push(html.getAttribute('data-theme') ?? '<unset>');
        });
        obs.observe(html, { attributes: true, attributeFilter: ['data-theme'] });
        setTimeout(() => { obs.disconnect(); resolve(seen); }, 800);
      });
    });
    for (const value of iframeTransitions) {
      expect(value, `iframe data-theme transitions: ${iframeTransitions.join(' → ')}`).toBe('light');
    }
  });
});

// Regression: theme integration is opt-in. An app served without the
// `<script src="/api/v1/sdk-prefs.js">` tag must NOT have `data-theme`
// injected on its <html>. The engine returns app HTML untouched — no
// auto-injection of script or stylesheet tags.

const NO_OPT_IN_APP_ID = 'e2e-sdk-no-opt-in';
let noOptInFixture: { cleanup: () => void };

test.describe('SDK iframe theme — opt-in only', () => {
  test.beforeAll(() => {
    noOptInFixture = createIframeAppFixture(NO_OPT_IN_APP_ID, {
      manifest: { id: NO_OPT_IN_APP_ID, name: 'No opt-in', description: 'e2e fixture' },
      // Plain HTML: no sdk-prefs.js script, no sdk-iframe.css link, nothing
      // that would pull in theme integration. The app's own styles must be
      // the only thing shaping its appearance.
      html: `<!DOCTYPE html>
<html>
<head>
<meta charset="UTF-8">
<title>No opt-in</title>
</head>
<body>
<div id="ready">ready</div>
</body>
</html>
`,
      js: '',
    });
  });

  test.afterAll(() => {
    noOptInFixture.cleanup();
  });

  test('app served without sdk-prefs.js does NOT receive data-theme on <html>', async ({ page, request }) => {
    // Origin matters — the iframe URL is path-relative.
    await gotoWithRetry(page, '/');
    await page.setContent(`<!DOCTYPE html>
<html>
<head><meta charset="UTF-8"></head>
<body>
<iframe id="app-frame" src="${appPath(NO_OPT_IN_APP_ID)}"
  sandbox="allow-scripts allow-same-origin"
  style="width:600px;height:400px;border:0"></iframe>
</body>
</html>`);

    const appFrame = page.frameLocator('#app-frame');
    await expect(appFrame.locator('#ready')).toBeVisible({ timeout: 5000 });
    await page.waitForTimeout(200);

    // The contract: opt-out apps see no engine-driven theme injection.
    await expect(appFrame.locator('html')).not.toHaveAttribute('data-theme', /.*/);

    // And the engine must NOT have rewritten the served HTML to include
    // the prefs script or theme stylesheet on this app's behalf.
    const res = await request.get(appPath(NO_OPT_IN_APP_ID));
    const html = await res.text();
    expect(html).not.toContain('/api/v1/sdk-prefs.js');
    expect(html).not.toContain('/api/v1/sdk-iframe.css');
  });
});

// Systemic regression: an app iframe rendered DARK even when the device was
// Light, for EVERY app, because `applyPreferences()` ran after sdk-prefs.js and
// resolved theme as `prefs['theme'] || 'dark'`. When the active device has no
// server-scoped `theme` (the reported iPhone-PWA case stores only `ui-scale`),
// that returned 'dark' and clobbered the correct Light value sdk-prefs.js had
// already applied from localStorage. The fix makes applyPreferences prefer the
// client value (localStorage / data-theme) over the hard default — so a missing
// server theme never flips the iframe to dark.

const NO_SERVER_THEME_APP_ID = 'e2e-sdk-no-server-theme';
let noServerThemeFixture: { cleanup: () => void };

test.describe('SDK iframe theme — localStorage wins when the server has no device-scoped theme', () => {
  test.beforeAll(() => {
    noServerThemeFixture = createIframeAppFixture(NO_SERVER_THEME_APP_ID, {
      manifest: { id: NO_SERVER_THEME_APP_ID, name: 'SDK no-server-theme test', description: 'e2e fixture' },
      // Standard app boilerplate: opt-in prefs script, shared stylesheet, SDK,
      // then applyPreferences + watchPreferences — the contract every app uses.
      html: `<!DOCTYPE html>
<html>
<head>
<meta charset="UTF-8">
<title>No server theme test</title>
<script src="/api/v1/sdk-prefs.js"></script>
<link rel="stylesheet" href="/api/v1/sdk-iframe.css">
<script src="/api/v1/sdk.js"></script>
</head>
<body>
<div id="ready">ready</div>
<script src="script.js"></script>
</body>
</html>
`,
      js: `
function waitForLucidos() {
  if (window.lucidos && window.lucidos.ui) {
    lucidos.ui.applyPreferences();
    lucidos.ui.watchPreferences();
  } else {
    setTimeout(waitForLucidos, 50);
  }
}
waitForLucidos();
`,
    });
  });

  test.afterAll(() => {
    noServerThemeFixture.cleanup();
    psql(`DELETE FROM preferences WHERE key = 'ui-scale'`);
  });

  test('iframe stays light from localStorage even though only ui-scale is stored server-side', async ({ page, request, context }) => {
    const deviceId = 'e2e-no-theme-' + Date.now();

    // Seed the user's persisted client state BEFORE first paint: device id,
    // a Light theme in localStorage (what sdk-prefs.js + the parent FOUC read),
    // and the open app so the auto-restore mounts the iframe on reload.
    await context.addInitScript(([id, appId]) => {
      localStorage.setItem('lucidos-device-id', id);
      localStorage.setItem('lucidos-theme', 'light');
      localStorage.setItem('app-window-open', appId);
    }, [deviceId, NO_SERVER_THEME_APP_ID]);

    // The exact reported shape: the device has ONLY a server-scoped ui-scale —
    // no `theme` row. (A fresh device id guarantees no pre-existing theme row.)
    await request.put(`/api/v1/preferences?key=ui-scale`, {
      data: { value: '125', device_id: deviceId },
    });

    await gotoWithRetry(page, '/');

    const iframeLoc = page.locator('iframe[data-role="app-ui-frame"]:visible');
    await expect(iframeLoc).toBeVisible({ timeout: 10_000 });

    const appFrame = page.frameLocator('iframe[data-role="app-ui-frame"]:visible');
    await expect(appFrame.locator('#ready')).toBeVisible({ timeout: 10_000 });

    // Capture every data-theme transition inside the iframe. The bug manifested
    // as a flip to `dark` AFTER applyPreferences ran (the async prefs fetch
    // resolved with no theme and overwrote the localStorage-light value). A
    // single post-load assertion can miss the brief dark frame.
    const transitions = await appFrame.locator('html').evaluate((html) => {
      return new Promise<string[]>((resolve) => {
        const seen: string[] = [html.getAttribute('data-theme') ?? '<unset>'];
        const obs = new MutationObserver(() => {
          seen.push(html.getAttribute('data-theme') ?? '<unset>');
        });
        obs.observe(html, { attributes: true, attributeFilter: ['data-theme'] });
        setTimeout(() => { obs.disconnect(); resolve(seen); }, 1000);
      });
    });
    for (const value of transitions) {
      expect(value, `iframe data-theme transitions: ${transitions.join(' → ')}`).toBe('light');
    }
  });
});
