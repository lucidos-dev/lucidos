import { test, expect } from './fixtures';
import { createIframeAppFixture } from './db-helpers';
import { gotoWithRetry } from './helpers';


const APP_ID = 'e2e-copy-app';
const PROBE = 'lucidos e2e clipboard probe';

let fixture: { dir: string; cleanup: () => void } | undefined;

test.describe('an app frame can copy', () => {
  test.beforeAll(() => {
    fixture = createIframeAppFixture(APP_ID, {
      manifest: { id: APP_ID, name: 'Copy app', description: 'e2e fixture' },
      html: `<!DOCTYPE html>
<html>
<head><meta charset="UTF-8"><title>Copy app</title></head>
<body>
<div id="ready">ready</div>
<button id="copy">Copy</button>
<div id="outcome">untried</div>
<script>
  // Unguarded on purpose: this is how app authors write a Copy button, and
  // the silence is half the defect. The outcome lands in the DOM so the spec
  // reads the browser's own verdict rather than inferring one.
  document.getElementById('copy').addEventListener('click', function () {
    navigator.clipboard.writeText(${JSON.stringify(PROBE)}).then(
      function () { document.getElementById('outcome').textContent = 'resolved'; },
      function (err) { document.getElementById('outcome').textContent = 'rejected ' + err.name + ': ' + err.message; }
    );
  });
</script>
</body>
</html>
`,
      js: '',
    });
  });

  test.afterAll(() => {
    fixture?.cleanup();
  });

  async function openAppOnLoad(page: import('@playwright/test').Page): Promise<void> {
    await page.addInitScript((id) => {
      if (window.parent !== window) return;
      localStorage.setItem('app-window-open', id);
    }, APP_ID);
  }

  test('a Copy button inside an app puts text on the clipboard', async ({ page }) => {
    await page.context().grantPermissions(['clipboard-write']);

    await openAppOnLoad(page);
    await gotoWithRetry(page, '/');

    const frameElement = page.locator('iframe[data-role="app-ui-frame"]:visible');
    await expect(frameElement).toHaveCount(1, { timeout: 15_000 });

    const allow = await frameElement.getAttribute('allow');
    expect(allow, 'the shipped frame must delegate clipboard-write').toContain('clipboard-write');
    expect(allow, 'reading the clipboard is deliberately not delegated').not.toContain('clipboard-read');

    const appFrame = page.frameLocator('iframe[data-role="app-ui-frame"]:visible');
    await expect(appFrame.locator('#ready')).toBeVisible({ timeout: 15_000 });

    await appFrame.locator('#copy').click();

    await expect(appFrame.locator('#outcome')).toHaveText('resolved');
  });
});
