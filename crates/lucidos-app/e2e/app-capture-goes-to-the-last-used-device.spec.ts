import { randomUUID } from 'crypto';
import { test, expect } from './fixtures';
import type { Page } from './fixtures';
import { createIframeAppFixture } from './db-helpers';
import { apiRequest, gotoWithRetry, shellDeviceId, waitForEventStream, waitForWorkspaceReady } from './helpers';

// A capture names one device, the turn's last used device, and only that
// page answers it. Every page gets the event, so a page without the app must
// never answer a capture meant for another device. See
// docs/plans/2026-10-08-app-capture-goes-to-the-last-used-device.md.
//
// Two browser contexts are two devices: each mints its own device id. The
// seam stands in for the agent's `capture_app` call; everything after it is
// the real path, through the sandboxed frame's own capture.

const APP_ID = 'e2e-capture-routing-app';
const DOM_PROBE = 'Capture routing probe';
/** The top half of the app, so a blank picture cannot pass for a real one. */
const SWATCH_RGB = [220, 40, 40];

let fixture: { dir: string; cleanup: () => void } | undefined;

/** Count the capture answers a page posts. */
function countAnswers(page: Page): { count: number } {
  const seen = { count: 0 };
  page.on('request', (r) => {
    if (r.method() === 'POST' && r.url().endsWith('/api/v1/app-capture')) seen.count++;
  });
  return seen;
}

test.describe('an app capture goes to the device it names', () => {
  test.beforeAll(() => {
    fixture = createIframeAppFixture(APP_ID, {
      manifest: { id: APP_ID, name: 'Capture routing app', description: 'e2e fixture' },
      html: `<!DOCTYPE html>
<html>
<head>
<meta charset="UTF-8">
<title>Capture routing app</title>
<script src="/api/v1/sdk.js"></script>
<style>
  html, body { margin: 0; height: 100%; background: #fff !important; }
  #swatch { position: absolute; left: 0; top: 0; width: 100%; height: 50%; background: rgb(${SWATCH_RGB.join(' ')}); }
  #probe { position: absolute; left: 20px; bottom: 20px; font: 16px sans-serif; color: #000; }
</style>
</head>
<body>
<div id="swatch"></div>
<div id="probe">${DOM_PROBE}</div>
</body>
</html>
`,
      js: '',
    });
  });

  test.afterAll(() => {
    fixture?.cleanup();
  });

  test('the named device answers, and a page without the app stays silent', async ({ page, browser, baseURL }) => {
    await page.addInitScript((id) => {
      // An init script runs in EVERY frame, and the app frame's storage throws.
      if (window.parent !== window) return;
      localStorage.setItem('app-window-open', id);
    }, APP_ID);
    await gotoWithRetry(page, '/');
    await waitForWorkspaceReady(page);
    const frameElement = page.locator(`iframe[data-role="app-ui-frame"][src*="${APP_ID}"]`).first();
    await expect(frameElement).toHaveCount(1, { timeout: 15_000 });
    expect(await frameElement.getAttribute('sandbox'), 'the frame must be opaque-origin').not.toContain(
      'allow-same-origin',
    );
    const frame = await (await frameElement.elementHandle())?.contentFrame();
    if (!frame) throw new Error('the app frame has no content');
    await expect(frame.locator('#probe')).toBeVisible({ timeout: 15_000 });

    const otherContext = await browser.newContext({ baseURL, ignoreHTTPSErrors: true });
    try {
      const other = await otherContext.newPage();
      await gotoWithRetry(other, '/');
      await waitForWorkspaceReady(other);
      await waitForEventStream(page, 30_000);
      await waitForEventStream(other, 30_000);

      const withApp = await shellDeviceId(page);
      const withoutApp = await shellDeviceId(other);
      expect(withoutApp, 'two contexts must be two devices').not.toBe(withApp);
      const answersWithApp = countAnswers(page);
      const answersWithoutApp = countAnswers(other);

      const ask = async (deviceId: string): Promise<string> => {
        const res = await apiRequest(page).post('/api/v1/internal/request-app-capture-for-test', {
          data: { thread_id: randomUUID(), app_id: APP_ID, device_id: deviceId },
          timeout: 20_000,
        });
        expect(res.status()).toBe(200);
        return ((await res.json()) as { result: string }).result;
      };

      const result = await ask(withApp);
      expect(result.slice(0, 200), 'the named device sent its capture').toMatch(/^\[APP_CAPTURE:/);
      expect(result, 'the capture carries the DOM').toContain(DOM_PROBE);
      expect(answersWithoutApp.count, 'the page without the app must not answer').toBe(0);
      expect(answersWithApp.count).toBe(1);

      const shot = /^\[APP_CAPTURE:([^\]]+)\]/.exec(result)?.[1] ?? '';
      const picture = await page.evaluate(async (b64) => {
        const img = new Image();
        img.src = `data:image/jpeg;base64,${b64}`;
        await img.decode();
        const canvas = document.createElement('canvas');
        canvas.width = img.width;
        canvas.height = img.height;
        const ctx = canvas.getContext('2d') as CanvasRenderingContext2D;
        ctx.drawImage(img, 0, 0);
        const at = (x: number, y: number) =>
          Array.from(ctx.getImageData(Math.round(x), Math.round(y), 1, 1).data.slice(0, 3));
        return {
          size: [img.width, img.height],
          swatch: at(img.width / 2, img.height / 4),
          below: at(img.width / 2, (img.height * 3) / 4),
        };
      }, shot);
      expect(picture.size[0], 'the picture has a width').toBeGreaterThan(0);
      for (let channel = 0; channel < 3; channel++) {
        expect(
          Math.abs(picture.swatch[channel] - SWATCH_RGB[channel]),
          `the swatch came out ${picture.swatch}, not ${SWATCH_RGB}: the picture is blank`,
        ).toBeLessThan(24);
      }
      expect(Math.min(...picture.below), `below the swatch should be white, got ${picture.below}`).toBeGreaterThan(200);

      // Named the other way, the page without the app answers, and its error
      // says which device it came from. The page with the app stays silent.
      const missed = await ask(withoutApp);
      expect(missed).toMatch(/^Error from ".+", the user's last used device in this turn: No app UI is currently open/);
      expect(answersWithApp.count, 'the page with the app must not answer a capture for another device').toBe(1);
    } finally {
      await otherContext.close();
    }
  });
});
