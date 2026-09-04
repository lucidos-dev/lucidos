/**
 * 100% in the image popup means one image pixel per PHYSICAL screen pixel.
 *
 * The popup used to count CSS pixels. A phone draws three device pixels per CSS
 * pixel. So a screenshot of that phone filled its screen at 33%, with every one
 * of its pixels on a screen pixel. The "actual size" the level control offered
 * from there was a threefold blow-up of the same pixels.
 *
 * Only a browser can check this. It is a measurement of the rendered image
 * against the screen it is rendered on. Both mobile projects run it, at one
 * device pixel per CSS pixel and at three, and each sizes its own fixture from
 * what it measures. So the arithmetic is never restated here, and the same two
 * assertions hold on both: an image the size of this screen reads 100%, and one
 * twice that reads 50% until actual size doubles it.
 *
 * The desktop half, where the fitted view and the zoom range are checked, is
 * preview-image-popup-desktop.spec.ts.
 */
import { test, expect, type Page } from './fixtures';
import {
  apiRequest, assertHealthy, clickVisibleElement, ensureOnThreadPane, gotoWithRetry,
  openFilesPanel, waitForVisibleElement, waitForVisibleInput,
} from './helpers';

/** The screen this project is emulating, as the popup measures it.
 *
 *  The fixture is sized off this, so the spec is mobile-only by necessity
 *  rather than by convention. The phone popup is 100vw by 100vh, so the window
 *  IS the container. The desktop popup is 95vw by 95vh, where the same fixture
 *  would read 95%. */
async function screenSize(page: Page): Promise<{ cssWidth: number; ratio: number }> {
  return page.evaluate(() => ({
    cssWidth: window.innerWidth,
    ratio: window.devicePixelRatio,
  }));
}

/** How wide the image is being drawn, in physical screen pixels, beside how
 *  many pixels it actually has. Equal is what 100% now claims. */
async function drawnAgainstNatural(page: Page): Promise<{ drawn: number; natural: number }> {
  return page.locator('.image-popup-slide img').first().evaluate((el) => {
    const img = el as HTMLImageElement;
    return {
      drawn: img.getBoundingClientRect().width * window.devicePixelRatio,
      natural: img.naturalWidth,
    };
  });
}

async function readoutPercent(page: Page): Promise<number> {
  return parseInt(await page.locator('.image-popup-zoom-level').innerText(), 10);
}

let created: string[] = [];

/** Put an image of `width` physical pixels in the workspace, then open it in
 *  the popup. It goes through the Files pane, the way a reader gets there.
 *
 *  The fixture is drawn on a canvas and PUT from the page. The whole
 *  measurement hangs off `naturalWidth`, and only a raster image reports it
 *  honestly. WebKit answers an SVG's `naturalWidth` with the size it was
 *  RENDERED at, so a 1170-wide SVG laid out across 390 CSS pixels reports 390.
 *  Chromium reports 1170. The two engines would be measuring different images.
 *
 *  The rectangle is far wider than any phone window is relative to its height,
 *  so the window's WIDTH is what fits it. The measurement then never depends on
 *  how tall the popup turns out to be. */
async function openImageOfWidth(page: Page, width: number): Promise<void> {
  const name = `e2e-popup-density-${width}-${Date.now()}.png`;
  const path = `artifacts/${name}`;
  const status = await page.evaluate(async ({ target, w }) => {
    const canvas = document.createElement('canvas');
    canvas.width = w;
    canvas.height = Math.round(w * 0.6);
    const ctx = canvas.getContext('2d');
    if (!ctx) return 0;
    ctx.fillStyle = '#123456';
    ctx.fillRect(0, 0, canvas.width, canvas.height);
    const blob = await new Promise<Blob | null>(done => canvas.toBlob(done, 'image/png'));
    if (!blob) return 0;
    // The device header the app's own `mutatingFetch` sends. A raw fetch is
    // an unidentified caller, which `PUT /api/v1/data/*path` refuses (ADR 0169).
    const deviceId = localStorage.getItem('lucidos-device-id');
    const resp = await fetch(`/api/v1/data/${target}`, {
      method: 'PUT',
      headers: {
        'Content-Type': 'image/png',
        ...(deviceId ? { 'x-lucidos-device-id': deviceId } : {}),
      },
      body: blob,
    });
    return resp.status;
  }, { target: path, w: width });
  created.push(path);
  // Exactly 200, never a range: the helper answers 0 when the canvas or the
  // blob is unavailable, and a range check would read that sentinel as a pass.
  expect(status, 'the fixture upload').toBe(200);

  await openFilesPanel(page);
  await waitForVisibleElement(page, '.file-item', 15_000);
  expect(await clickVisibleElement(page, '.file-item', name)).toBe(true);

  const preview = page.locator('.preview-image:visible').first();
  await expect(preview).toBeVisible({ timeout: 10_000 });
  await preview.click();
  await expect(page.locator('.image-popup-content')).toBeVisible({ timeout: 5_000 });

  // A transform is written once the popup has measured the loaded image. Until
  // then the level control reads the unmeasured default of 100%, which an
  // assertion expecting 100% would pass against without measuring anything.
  await page.waitForFunction(() => {
    const img = document.querySelector('.image-popup-slide img') as HTMLImageElement | null;
    return !!img && img.complete && img.style.transform !== '';
  }, undefined, { timeout: 10_000 });
  expect((await drawnAgainstNatural(page)).natural, 'the fixture arrived intact')
    .toBe(width);
}

test.describe('the image popup counts physical screen pixels', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    await gotoWithRetry(page, '/');
    await page.waitForFunction(() =>
      document.querySelector('#app')?.childElementCount! > 0,
      undefined, { timeout: 30_000 },
    );
    await ensureOnThreadPane(page);
    await waitForVisibleInput(page);
  });

  test.afterEach(async ({ page }) => {
    for (const path of created) await apiRequest(page).delete(`/api/v1/data/${path}`);
    created = [];
  });

  test('a capture of this screen, opened on it, is already actual size', async ({ page }) => {
    const { cssWidth, ratio } = await screenSize(page);
    await openImageOfWidth(page, Math.round(cssWidth * ratio));

    // Both ends of the level control are this one place. It says so, rather
    // than promising a zoom that would land where the image already is. This
    // is asserted first because only a measured level can disable it.
    await expect(page.locator('.image-popup-zoom-level')).toBeDisabled();
    await expect(page.locator('.image-popup-zoom-btn[aria-label="Zoom out"]')).toBeDisabled();
    await expect(page.locator('.image-popup-zoom-btn[aria-label="Zoom in"]')).toBeEnabled();

    // The reported bug read 33% here, on a phone drawing three pixels per CSS
    // pixel. The image is as sharp as the screen can draw it, which is 100%.
    expect(await readoutPercent(page)).toBe(100);
    const { drawn, natural } = await drawnAgainstNatural(page);
    expect(Math.abs(drawn - natural), 'one image pixel per screen pixel').toBeLessThan(2);

    await page.locator('.floating-mobile-close').click();
    await expect(page.locator('.image-popup-content')).toHaveCount(0);
  });

  test('twice this screen reads 50%, and actual size doubles it', async ({ page }) => {
    const { cssWidth, ratio } = await screenSize(page);
    await openImageOfWidth(page, Math.round(cssWidth * ratio * 2));

    await expect.poll(() => readoutPercent(page), { timeout: 5_000 }).toBe(50);

    const level = page.locator('.image-popup-zoom-level');
    await expect(level).toBeEnabled();
    await level.click();
    await expect.poll(() => readoutPercent(page), { timeout: 5_000 }).toBe(100);
    const { drawn, natural } = await drawnAgainstNatural(page);
    expect(Math.abs(drawn - natural), 'one image pixel per screen pixel').toBeLessThan(2);

    // And the same control puts it back where it opened.
    await level.click();
    await expect.poll(() => readoutPercent(page), { timeout: 5_000 }).toBe(50);
  });
});
