import { test, expect } from '@playwright/test';
import { createIframeAppFixture, psql } from './db-helpers';

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
    await page.goto('/');
    const deviceId = 'e2e-device-' + Date.now();
    await page.evaluate((id) => localStorage.setItem('lucidos-device-id', id), deviceId);
    await request.put(`/api/preferences?key=theme`, {
      data: { value: 'dark', device_id: deviceId },
    });

    // Embed the test app inside an iframe sandbox identical to AppUiInline.tsx.
    // The parent page also opens its own SSE listener (matches the real app
    // where Lucidos UI subscribes to the same /api/events).
    await page.setContent(`<!DOCTYPE html>
<html>
<head><meta charset="UTF-8"></head>
<body>
<iframe
  id="app-frame"
  src="/api/app/${APP_ID}/"
  sandbox="allow-scripts allow-same-origin allow-popups allow-forms allow-modals allow-popups-to-escape-sandbox"
  style="width:600px;height:400px;border:0"></iframe>
<script>
  // Mirror the parent Lucidos app — open an EventSource so connection-limit
  // contention with the iframe is realistic.
  window.__parentSse = new EventSource('/api/events');
</script>
</body>
</html>`);

    const appFrame = page.frameLocator('#app-frame');
    await expect(appFrame.locator('#status')).toHaveText('applied');
    await expect(appFrame.locator('html')).toHaveAttribute('data-theme', 'dark');

    // Toggle to light — SSE event broadcast to all subscribers including the iframe.
    await request.put(`/api/preferences?key=theme`, {
      data: { value: 'light', device_id: deviceId },
    });

    // Live update inside the iframe — should land within seconds.
    await expect(appFrame.locator('html')).toHaveAttribute('data-theme', 'light', { timeout: 5000 });
  });
});
