import { test, expect } from './fixtures';
import { appPath, createIframeAppFixture, psql } from './db-helpers';
import { gotoWithRetry, hideDashboardRail } from './helpers';


const APP_ID = 'e2e-sdk-theme-test';
let fixture: { cleanup: () => void };

test.describe('SDK iframe theme — live PreferencesChanged update', () => {
  test.beforeAll(() => {
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
    psql(`DELETE FROM preferences WHERE key = 'theme-mode'`);
  });

  test('app inside Lucidos iframe receives live PreferencesChanged via SSE', async ({ page, request }) => {
    await gotoWithRetry(page, '/');
    const deviceId = 'e2e-device-' + Date.now();
    await page.evaluate((id) => localStorage.setItem('lucidos-device-id', id), deviceId);
    await request.put(`/api/v1/preferences?key=theme-mode`, {
      data: { value: 'dark', device_id: deviceId },
    });

    await page.setContent(`<!DOCTYPE html>
<html>
<head><meta charset="UTF-8"></head>
<body>
<iframe
  id="app-frame"
  src="${appPath(APP_ID)}"
  sandbox="allow-scripts allow-same-origin allow-popups allow-forms allow-modals allow-popups-to-escape-sandbox allow-downloads"
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
    await expect(appFrame.locator('html')).toHaveAttribute('data-theme-mode', 'dark');

    await request.put(`/api/v1/preferences?key=theme-mode`, {
      data: { value: 'light', device_id: deviceId },
    });

    await expect(appFrame.locator('html')).toHaveAttribute('data-theme-mode', 'light', { timeout: 5000 });
  });

  test('opt-in /api/v1/sdk-prefs.js serves the boot script, and seeds it per device', async ({ request }) => {
    const res = await request.get('/api/v1/sdk-prefs.js');
    expect(res.status()).toBe(200);
    expect(res.headers()['content-type']).toContain('application/javascript');

    const js = await res.text();
    expect(js).toContain('GENERATED from packages/lucidos-sdk/src/boot/');
    expect(js).toContain('(() => {');
    expect(js).toContain('THEME_MODE_KEY = PREF_THEME_MODE.key');
    expect(js).toContain('THEME_MODE_STORAGE_KEY = "lucidos-theme-mode"');
    expect(js).toContain('seeded(served, THEME_MODE_KEY, THEME_MODE_STORAGE_KEY)');
    expect(js).toContain('seeded(served, PREF_FONT_FAMILY.key, "lucidos-font-family")');
    expect(js).toContain('wsLocalGet("lucidos-ui-scale")');
    expect(js).toContain('globalThis.__lucidosPrefs');
    expect(js).not.toContain('localStorage.getItem("lucidos-theme-mode")');
    expect(js).not.toContain('window.__lucidosPrefs=');
    expect(js).toContain('matchMedia("(prefers-color-scheme: light)")');
    expect(js).toContain('THEME_MODE_ATTRIBUTE = "data-theme-mode"');
    expect(js).toContain('setAttribute(THEME_MODE_ATTRIBUTE, resolved)');
    expect(js).toContain('setProperty("--bg-primary"');
    expect(js).toContain('setProperty("--font-ui"');
  });
});


const COLD_APP_ID = 'e2e-sdk-cold-load';
let coldFixture: { cleanup: () => void };

test.describe('SDK iframe theme — cold reload (full Lucidos bootstrap)', () => {
  test.beforeAll(() => {
    coldFixture = createIframeAppFixture(COLD_APP_ID, {
      manifest: { id: COLD_APP_ID, name: 'SDK cold-load test', description: 'e2e fixture' },
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
    psql(`DELETE FROM preferences WHERE key = 'theme-mode'`);
  });

  test('parent and iframe paint data-theme-mode="light" from frame zero, driven by localStorage', async ({ page, request, context }) => {
    const deviceId = 'e2e-cold-' + Date.now();

    await context.addInitScript(([id, appId]) => {
      localStorage.setItem('lucidos-device-id', id);
      localStorage.setItem('lucidos-theme-mode', 'light');
      localStorage.setItem('app-window-open', appId);
    }, [deviceId, COLD_APP_ID]);

    await request.put(`/api/v1/preferences?key=theme-mode`, {
      data: { value: 'light', device_id: deviceId },
    });

    await gotoWithRetry(page, '/');

    const iframeLoc = page.locator('iframe[data-role="app-ui-frame"]:visible');
    await expect(iframeLoc).toBeVisible({ timeout: 10_000 });

    const iframeBg = await page.evaluate(() =>
      getComputedStyle(document.querySelector('iframe[data-role="app-ui-frame"]')!).backgroundColor);
    expect(iframeBg, `iframe element bg must be white, not dark (got ${iframeBg})`).toMatch(/^rgba?\(\s*255\s*,\s*255\s*,\s*255/);

    const inlinedBgPrimary = await page.evaluate(() =>
      document.documentElement.style.getPropertyValue('--bg-primary'));
    expect(
      inlinedBgPrimary,
      'parent shell must inline --bg-primary on <html> from localStorage so the body var() resolves before external CSS loads',
    ).toBe('#ffffff');

    const appFrame = page.frameLocator('iframe[data-role="app-ui-frame"]:visible');
    await expect(appFrame.locator('#ready')).toBeVisible({ timeout: 10_000 });

    const iframeTransitions = await appFrame.locator('html').evaluate((html) => {
      return new Promise<string[]>((resolve) => {
        const seen: string[] = [html.getAttribute('data-theme-mode') ?? '<unset>'];
        const obs = new MutationObserver(() => {
          seen.push(html.getAttribute('data-theme-mode') ?? '<unset>');
        });
        obs.observe(html, { attributes: true, attributeFilter: ['data-theme-mode'] });
        setTimeout(() => { obs.disconnect(); resolve(seen); }, 800);
      });
    });
    for (const value of iframeTransitions) {
      expect(value, `iframe data-theme-mode transitions: ${iframeTransitions.join(' → ')}`).toBe('light');
    }
  });
});


const NO_OPT_IN_APP_ID = 'e2e-sdk-no-opt-in';
let noOptInFixture: { cleanup: () => void };

test.describe('SDK iframe theme — opt-in only', () => {
  test.beforeAll(() => {
    noOptInFixture = createIframeAppFixture(NO_OPT_IN_APP_ID, {
      manifest: { id: NO_OPT_IN_APP_ID, name: 'No opt-in', description: 'e2e fixture' },
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

  test('app served without sdk-prefs.js does NOT receive data-theme-mode on <html>', async ({ page, request }) => {
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

    await expect(appFrame.locator('html')).not.toHaveAttribute('data-theme-mode', /.*/);

    const res = await request.get(appPath(NO_OPT_IN_APP_ID));
    const html = await res.text();
    expect(html).not.toContain('/api/v1/sdk-prefs.js');
    expect(html).not.toContain('/api/v1/sdk-iframe.css');
  });
});


const NO_SERVER_THEME_APP_ID = 'e2e-sdk-no-server-theme';
let noServerThemeFixture: { cleanup: () => void };

test.describe('SDK iframe theme — localStorage wins when the server has no device-scoped theme', () => {
  test.beforeAll(() => {
    noServerThemeFixture = createIframeAppFixture(NO_SERVER_THEME_APP_ID, {
      manifest: { id: NO_SERVER_THEME_APP_ID, name: 'SDK no-server-theme test', description: 'e2e fixture' },
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

    await hideDashboardRail(page);
    await context.addInitScript(([id, appId]) => {
      localStorage.setItem('lucidos-device-id', id);
      localStorage.setItem('lucidos-theme-mode', 'light');
      localStorage.setItem('app-window-open', appId);
    }, [deviceId, NO_SERVER_THEME_APP_ID]);

    await request.put(`/api/v1/preferences?key=ui-scale`, {
      data: { value: '125', device_id: deviceId },
    });

    await gotoWithRetry(page, '/');

    const iframeLoc = page.locator('iframe[data-role="app-ui-frame"]:visible');
    await expect(iframeLoc).toBeVisible({ timeout: 10_000 });

    const appFrame = page.frameLocator('iframe[data-role="app-ui-frame"]:visible');
    await expect(appFrame.locator('#ready')).toBeVisible({ timeout: 10_000 });

    const transitions = await appFrame.locator('html').evaluate((html) => {
      return new Promise<string[]>((resolve) => {
        const seen: string[] = [html.getAttribute('data-theme-mode') ?? '<unset>'];
        const obs = new MutationObserver(() => {
          seen.push(html.getAttribute('data-theme-mode') ?? '<unset>');
        });
        obs.observe(html, { attributes: true, attributeFilter: ['data-theme-mode'] });
        setTimeout(() => { obs.disconnect(); resolve(seen); }, 1000);
      });
    });
    for (const value of transitions) {
      expect(value, `iframe data-theme-mode transitions: ${transitions.join(' → ')}`).toBe('light');
    }
  });
});
