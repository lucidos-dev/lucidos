import { readFileSync, rmSync } from 'fs';
import { resolve } from 'path';
import { fileURLToPath } from 'url';
import { test, expect } from './fixtures';
import { WORKSPACE, createIframeAppFixture } from './db-helpers';
import { apiRequest, assertHealthy, navigateToApp, waitForEventStream } from './helpers';

test.use({ viewport: { width: 1280, height: 900 } });

const APP_ID = 'e2e-workspace-font-app';

const FONT_BYTES = readFileSync(
  fileURLToPath(new URL('../src/assets/fonts/FiraCode-VF.woff2', import.meta.url)),
);

let fixture: { dir: string; cleanup: () => void } | undefined;
let fontDir: string | undefined;

test.describe('a workspace font', () => {
  test.beforeAll(() => {
    fixture = createIframeAppFixture(APP_ID, {
      manifest: { id: APP_ID, name: 'Workspace font app', description: 'e2e fixture' },
      html: `<!DOCTYPE html>
<html>
<head>
<meta charset="UTF-8">
<script src="/api/v1/sdk-prefs.js"></script>
<link rel="stylesheet" href="/api/v1/sdk-iframe.css">
<script src="/api/v1/sdk.js"></script>
</head>
<body>
<div id="fonts">pending</div>
<script src="script.js"></script>
</body>
</html>
`,
      js: `
lucidos.ui.applyPreferences().then(function () {
  var ui = getComputedStyle(document.documentElement).getPropertyValue('--font-ui');
  var family = ui.split(',')[0].trim().replace(/["']/g, '');
  return document.fonts.load("16px '" + family + "'").then(function () {
    var faces = [];
    document.fonts.forEach(function (face) {
      if (face.family.replace(/["']/g, '') === family) faces.push(family + ':' + face.status);
    });
    document.getElementById('fonts').textContent = faces.join(',') || ('no face for ' + family);
  });
}).catch(function (err) {
  document.getElementById('fonts').textContent = String(err);
});
`,
    });
  });

  test.afterAll(() => {
    fixture?.cleanup();
    if (fontDir) rmSync(fontDir, { recursive: true, force: true });
  });

  test('installs from Settings, paints the shell and an app frame, and loads only locally', async ({ page }) => {
    await assertHealthy(page);
    const fontRequests: string[] = [];
    page.on('request', (req) => {
      if (req.resourceType() === 'font') fontRequests.push(req.url());
    });

    await navigateToApp(page);
    const origin = new URL(page.url()).origin;
    await waitForEventStream(page);
    const nav = await apiRequest(page).post('/api/v1/ui/navigate', {
      headers: { 'content-type': 'application/json' },
      data: { target: 'settings', params: { settings_view: 'appearance' } },
    });
    expect(nav.ok(), `POST /api/v1/ui/navigate -> ${nav.status()}`).toBeTruthy();

    const label = `E2e Face ${Date.now()}`;
    const slug = label.toLowerCase().replace(/[^a-z0-9]+/g, '-');
    const family = `ws-${slug}`;
    fontDir = resolve(WORKSPACE, 'data/fonts', slug);

    await page.getByRole('button', { name: 'Install font' }).click();
    const form = page.locator('.workspace-font-form');
    await form.locator('input[type="file"]').setInputFiles({
      name: 'E2eFace-Regular.woff2',
      mimeType: 'font/woff2',
      buffer: FONT_BYTES,
    });
    await form.locator('input[placeholder="Brand Sans"]').fill(label);
    await form.getByRole('button', { name: 'Install', exact: true }).click();

    const row = page.locator('.workspace-font-row', { hasText: label });
    await expect(row).toBeVisible({ timeout: 15_000 });
    await row.getByRole('button', { name: 'Use' }).click();

    await expect
      .poll(() => page.evaluate(() => getComputedStyle(document.documentElement).getPropertyValue('--font-ui')))
      .toContain(`'${family}'`);
    const status = await page.evaluate(async (f) => {
      await document.fonts.load(`16px '${f}'`);
      const faces: string[] = [];
      document.fonts.forEach((face) => {
        if (face.family.replace(/["']/g, '') === f) faces.push(face.status);
      });
      return faces.join(',');
    }, family);
    expect(status).toBe('loaded');

    await page.addInitScript((id) => {
      if (window.parent !== window) return;
      localStorage.removeItem('lucidos-nav-history');
      localStorage.setItem('app-window-open', id);
    }, APP_ID);
    await page.reload();
    const frameSelector = `iframe[data-role="app-ui-frame"][src*="${APP_ID}"]`;
    await expect(page.locator(frameSelector).first()).toHaveCount(1, { timeout: 15_000 });
    await expect(page.frameLocator(frameSelector).first().locator('#fonts')).toHaveText(`${family}:loaded`, {
      timeout: 15_000,
    });

    const faceRequests = fontRequests.filter((url) => url.includes(`/data/fonts/${slug}/`));
    expect(faceRequests.length, 'the workspace face was requested').toBeGreaterThan(0);
    for (const url of fontRequests) {
      expect(new URL(url).origin, `${url} left the local engine`).toBe(origin);
    }
  });
});
