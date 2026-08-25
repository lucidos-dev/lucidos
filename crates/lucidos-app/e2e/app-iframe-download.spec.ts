import { writeFileSync } from 'fs';
import { resolve } from 'path';
import { test, expect } from './fixtures';
import { createIframeAppFixture } from './db-helpers';
import { gotoWithRetry } from './helpers';

// Regression for the bug where every app-UI download was silently blocked.
// AppUiInline.tsx rendered the app iframe with no `allow-downloads`. Chrome
// refused any download navigation from inside it: a same-origin
// `<a download>`, a `blob:` object URL, or a `data:` URI. The app had no way
// to observe the refusal, so a click just did nothing.
//
// This opens a real app through the same restore path sdk-iframe-mount.spec.ts
// uses. The iframe then carries AppUiInline's actual production `sandbox`
// attribute, not a string copied into the test.

const APP_ID = 'e2e-download-test';
let fixture: { dir: string; cleanup: () => void };

test.describe('App iframe downloads', () => {
  test.beforeAll(() => {
    fixture = createIframeAppFixture(APP_ID, {
      manifest: { id: APP_ID, name: 'Download test', description: 'e2e fixture' },
      html: `<!DOCTYPE html>
<html>
<head><meta charset="UTF-8"><title>Download test</title></head>
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
    // A same-origin file for the plain `<a download>` case, served statically
    // alongside index.html via the app's `/*path` route.
    writeFileSync(resolve(fixture.dir, 'payload.txt'), 'from a same-origin file');
  });

  test.afterAll(() => {
    fixture.cleanup();
  });

  // Mirrors sdk-iframe-mount.spec.ts: seeding `app-window-open` before
  // navigation makes loadApps()'s restore branch mount the app via the real
  // AppUiInline component. The sandbox is production code, not a copied
  // string.
  async function openAppOnLoad(page: import('@playwright/test').Page): Promise<void> {
    await page.addInitScript((id) => {
      localStorage.setItem('app-window-open', id);
    }, APP_ID);
  }

  test('same-origin, blob:, and data: downloads all fire from inside the app iframe', async ({ page }, testInfo) => {
    // WebKit's Playwright driver does not reliably fire a `download` event for
    // ANY anchor inside a sandboxed iframe, fix or no fix (verified: the
    // same-origin case times out identically on mobile-webkit either way).
    // That's a WebKit/Playwright gap, not the Chrome sandbox bug this spec
    // guards against, so it's out of scope here.
    testInfo.skip(testInfo.project.name === 'mobile-webkit', 'WebKit does not fire download events from inside a sandboxed iframe');

    await openAppOnLoad(page);
    await gotoWithRetry(page, '/');

    const iframeLoc = page.locator('iframe[data-role="app-ui-frame"]:visible');
    await expect(iframeLoc).toBeVisible({ timeout: 10_000 });
    const appFrame = page.frameLocator('iframe[data-role="app-ui-frame"]:visible');
    await expect(appFrame.locator('#ready')).toBeVisible({ timeout: 10_000 });

    // 10s, not the usual 5: Chromium's download subsystem takes a one-time
    // beat to initialize on the first download of a fresh browser instance,
    // which this is. A real regression never fires the event at all, so a
    // looser timeout here doesn't weaken the assertion.
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
