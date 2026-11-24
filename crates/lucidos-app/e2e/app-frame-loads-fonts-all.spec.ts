import { readFileSync } from 'fs';
import { fileURLToPath } from 'url';
import { test, expect } from './fixtures';
import { createIframeAppFixture } from './db-helpers';
import { gotoWithRetry } from './helpers';


const APP_ID = 'e2e-font-app';

const FONT_BYTES = readFileSync(
  fileURLToPath(new URL('../src/assets/fonts/FiraCode-VF.woff2', import.meta.url)),
);

let fixture: { dir: string; cleanup: () => void } | undefined;

test.describe('an app frame loads fonts', () => {
  test.beforeAll(() => {
    fixture = createIframeAppFixture(APP_ID, {
      manifest: { id: APP_ID, name: 'Font app', description: 'e2e fixture' },
      html: `<!DOCTYPE html>
<html>
<head>
<meta charset="UTF-8">
<title>Font app</title>
<script src="/api/v1/sdk-prefs.js"></script>
<link rel="stylesheet" href="/api/v1/sdk-iframe.css">
<script src="/api/v1/sdk.js"></script>
<style>
  @font-face { font-family: 'Own Face'; src: url(own.woff2) format('woff2'); }
</style>
</head>
<body>
<pre style="font-family: 'Own Face'">own</pre>
<div id="fonts">pending</div>
<script src="script.js"></script>
</body>
</html>
`,
      js: `
function sheetLoaded() {
  var link = document.querySelector('link[href*="fonts/fira-code.css"]');
  if (!link) return Promise.reject(new Error('no Fira Code stylesheet was linked'));
  if (link.sheet) return Promise.resolve();
  return new Promise(function (resolve, reject) {
    link.addEventListener('load', resolve, { once: true });
    link.addEventListener('error', function () { reject(new Error('the Fira Code stylesheet failed')); }, { once: true });
  });
}
lucidos.ui.applyPreferences().then(sheetLoaded).then(function () {
  return Promise.allSettled([
    document.fonts.load("16px 'Fira Code'"),
    document.fonts.load("16px 'Own Face'"),
  ]);
}).then(function () {
  var faces = [];
  document.fonts.forEach(function (face) {
    faces.push(face.family.replace(/["']/g, '') + ':' + face.status);
  });
  document.getElementById('fonts').textContent = faces.sort().join(',');
}).catch(function (err) {
  document.getElementById('fonts').textContent = String(err);
});
`,
      extra: { 'own.woff2': FONT_BYTES },
    });
  });

  test.afterAll(() => {
    fixture?.cleanup();
  });

  test('Fira Code and the app\'s own font both load', async ({ page }) => {
    const refusals: string[] = [];
    page.on('console', (msg) => {
      if (/CORS|Access-Control-Allow-Origin/i.test(msg.text())) refusals.push(msg.text());
    });

    await page.addInitScript((id) => {
      if (window.parent !== window) return;
      localStorage.setItem('app-window-open', id);
    }, APP_ID);
    await gotoWithRetry(page, '/');

    const frameSelector = `iframe[data-role="app-ui-frame"][src*="${APP_ID}"]`;
    const frameElement = page.locator(frameSelector).first();
    await expect(frameElement).toHaveCount(1, { timeout: 15_000 });
    expect(await frameElement.getAttribute('sandbox'), 'the frame must be opaque-origin').not.toContain(
      'allow-same-origin',
    );

    const appFrame = page.frameLocator(frameSelector).first();
    await expect(appFrame.locator('#fonts')).toHaveText('Fira Code:loaded,Own Face:loaded', {
      timeout: 15_000,
    });
    expect(refusals, 'no CORS refusal may reach the console').toEqual([]);
  });
});
