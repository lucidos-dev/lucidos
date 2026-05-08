import { test, expect } from '@playwright/test';
import { appPath, createIframeAppFixture } from './db-helpers';

// Verifies that the SDK preserves an app's scrollY across iframe unmount/remount.
// The parent destroys/recreates the iframe element on every app switch
// (AppUiInline.tsx uses key={refreshKey}), so without intervention the user
// loses their place every time. The SDK saves on `pagehide` and restores on
// load, keyed by app id in sessionStorage.

const APP_ID = 'e2e-sdk-scroll-test';
let fixture: { cleanup: () => void };

// Sandbox attrs must mirror AppUiInline.tsx — keep them identical so the test
// exercises the real iframe environment.
const iframeHtml = (appId: string) => `<!DOCTYPE html>
<html><body>
<iframe id="app-frame" src="${appPath(appId)}"
  sandbox="allow-scripts allow-same-origin allow-popups allow-forms allow-modals allow-popups-to-escape-sandbox"
  style="width:600px;height:400px;border:0"></iframe>
</body></html>`;

test.describe('SDK iframe scroll memory', () => {
  test.beforeAll(() => {
    fixture = createIframeAppFixture(APP_ID, {
      manifest: { id: APP_ID, name: 'SDK scroll test', description: 'e2e fixture' },
      html: `<!DOCTYPE html>
<html>
<head>
<meta charset="UTF-8">
<title>Scroll test</title>
<script src="/api/v1/sdk.js"></script>
<style>body { margin: 0; padding: 0; } .row { height: 50px; border-bottom: 1px solid #ccc; }</style>
</head>
<body>
<script src="script.js"></script>
</body>
</html>
`,
      js: `
// Render 200 rows = ~10000px tall. Tall enough to scroll meaningfully.
for (let i = 0; i < 200; i++) {
  const div = document.createElement('div');
  div.className = 'row';
  div.textContent = 'Row ' + i;
  document.body.appendChild(div);
}
`,
    });
  });

  test.afterAll(() => {
    fixture.cleanup();
  });

  test('restores scrollY when sessionStorage already has a value for this app', async ({ page }) => {
    // Pre-seed the per-app key from the parent (same-origin iframe shares
    // sessionStorage with parent). This isolates the restore path from the
    // save path, which is independently exercised in the next test.
    await page.goto('/');
    await page.evaluate((id) => {
      sessionStorage.setItem(`lucidos-scroll-app-${id}`, '1000');
    }, APP_ID);

    await page.setContent(iframeHtml(APP_ID));

    const appFrame = page.frameLocator('#app-frame');
    await expect(appFrame.locator('.row').nth(199)).toBeVisible({ timeout: 5000 });

    // SDK restore is async (waits for body to grow via MutationObserver).
    // Poll until restored or timeout.
    await expect.poll(async () => {
      return await page.evaluate(() => {
        const iframe = document.querySelector('#app-frame') as HTMLIFrameElement;
        return iframe.contentWindow!.scrollY;
      });
    }, { timeout: 3000 }).toBe(1000);
  });

  test('saves scrollY on pagehide so the next mount restores it', async ({ page }) => {
    await page.goto('/');
    // Clear any stored value from a previous test.
    await page.evaluate((id) => {
      sessionStorage.removeItem(`lucidos-scroll-app-${id}`);
    }, APP_ID);

    await page.setContent(iframeHtml(APP_ID));

    await expect(page.frameLocator('#app-frame').locator('.row').nth(199)).toBeVisible({ timeout: 5000 });

    // Scroll the iframe content from inside the iframe (avoids cross-frame
    // scrollTo flakiness). Read scrollY back to confirm.
    const observedScrollY = await page.evaluate(() => {
      const iframe = document.querySelector('#app-frame') as HTMLIFrameElement;
      const win = iframe.contentWindow!;
      win.document.documentElement.scrollTop = 1000;
      return win.scrollY;
    });
    expect(observedScrollY).toBeGreaterThan(500);

    // Remove iframe — SDK's pagehide listener fires and writes scrollY
    // to sessionStorage. The exact value depends on observedScrollY (may be
    // clamped if content is shorter than expected).
    await page.evaluate(() => {
      document.querySelector('#app-frame')?.remove();
    });

    const saved = await page.evaluate((id) => {
      return sessionStorage.getItem(`lucidos-scroll-app-${id}`);
    }, APP_ID);
    expect(Number(saved)).toBeGreaterThan(500);
  });

  test('does not bleed scroll across different apps', async ({ page }) => {
    const otherId = APP_ID + '-other';
    const other = createIframeAppFixture(otherId, {
      manifest: { id: otherId, name: 'Other app', description: 'e2e fixture' },
      html: `<!DOCTYPE html>
<html>
<head><meta charset="UTF-8"><script src="/api/v1/sdk.js"></script>
<style>body { margin: 0; padding: 0; } .row { height: 50px; }</style></head>
<body><script src="script.js"></script></body>
</html>`,
      js: `
for (let i = 0; i < 200; i++) {
  const d = document.createElement('div');
  d.className = 'row';
  d.textContent = 'B ' + i;
  document.body.appendChild(d);
}
`,
    });

    try {
      await page.goto('/');
      // Seed scroll memory for the FIRST app only.
      await page.evaluate((id) => {
        sessionStorage.setItem(`lucidos-scroll-app-${id}`, '1000');
      }, APP_ID);

      // Mount the OTHER app's iframe — its own key has nothing saved, so it
      // must start at 0 even though the first app's key is in storage.
      await page.setContent(iframeHtml(otherId));

      await expect(page.frameLocator('#app-frame').locator('.row').nth(199)).toBeVisible({ timeout: 5000 });
      // Give any (incorrect) deferred restore a chance to fire.
      await page.waitForTimeout(500);

      const otherY = await page.evaluate(() => {
        return (document.querySelector('#app-frame') as HTMLIFrameElement).contentWindow!.scrollY;
      });
      expect(otherY).toBe(0);
    } finally {
      other.cleanup();
    }
  });
});
