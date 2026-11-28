import { writeFileSync } from 'fs';
import { resolve } from 'path';
import { test, expect } from './fixtures';
import { createIframeAppFixture } from './db-helpers';
import { gotoWithRetry } from './helpers';


const APP_ID = 'e2e-download-test';
let fixture: { dir: string; cleanup: () => void };

test.describe('App iframe downloads', () => {
  test.beforeAll(() => {
    fixture = createIframeAppFixture(APP_ID, {
      manifest: { id: APP_ID, name: 'Download test', description: 'e2e fixture' },
      html: `<!DOCTYPE html>
<html>
<head><meta charset="UTF-8"><title>Download test</title>
<script src="/api/v1/sdk.js"></script>
</head>
<body>
<div id="ready">ready</div>
<a id="same-origin-download" href="payload.txt" download="payload.txt">Same-origin</a>
<a id="blob-download" download="blob.txt">Blob</a>
<a id="data-download" href="data:text/plain;base64,ZnJvbSBhIGRhdGEgVVJJ" download="data.txt">Data URI</a>
<script>
document.getElementById('blob-download').addEventListener('click', function () {
  var blob = new Blob(['from a blob URL'], { type: 'text/plain' });
  this.href = URL.createObjectURL(blob);
});
</script>
</body>
</html>
`,
      js: '',
    });
    writeFileSync(resolve(fixture.dir, 'payload.txt'), 'from a same-origin file');
  });

  test.afterAll(() => {
    fixture.cleanup();
  });

  async function openAppOnLoad(page: import('@playwright/test').Page): Promise<void> {
    await page.addInitScript((id) => {
      localStorage.setItem('app-window-open', id);
    }, APP_ID);
  }

  test('same-origin, blob:, and data: downloads all fire from inside the app iframe', async ({ page }) => {
    await openAppOnLoad(page);
    await gotoWithRetry(page, '/');

    const iframeLoc = page.locator('iframe[data-role="app-ui-frame"]:visible');
    await expect(iframeLoc).toBeVisible({ timeout: 10_000 });
    const appFrame = page.frameLocator('iframe[data-role="app-ui-frame"]:visible');
    await expect(appFrame.locator('#ready')).toBeVisible({ timeout: 10_000 });
    await expect(page.locator('.app-ui-cover')).toHaveCount(0, { timeout: 10_000 });

    const [sameOriginDownload] = await Promise.all([
      page.waitForEvent('download', { timeout: 10_000 }),
      appFrame.locator('#same-origin-download').click(),
    ]);
    expect(sameOriginDownload.suggestedFilename()).toBe('payload.txt');

    const [blobDownload] = await Promise.all([
      page.waitForEvent('download', { timeout: 5000 }),
      appFrame.locator('#blob-download').click(),
    ]);
    expect(blobDownload.suggestedFilename()).toBe('blob.txt');

    const [dataUriDownload] = await Promise.all([
      page.waitForEvent('download', { timeout: 5000 }),
      appFrame.locator('#data-download').click(),
    ]);
    expect(dataUriDownload.suggestedFilename()).toBe('data.txt');
  });
});
