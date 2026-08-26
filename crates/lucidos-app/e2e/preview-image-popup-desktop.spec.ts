/**
 * A previewed image opens full size, and the popup it opens into is usable.
 *
 * The Files pane scales an image to fit, so a tall screenshot arrives as an
 * unreadable strip. Clicking it hands the image to the image popup, where the
 * zoom controls, a wheel and a pinch take over. That click was gated on a
 * mobile viewport, which left a desktop reader with no way in at all.
 *
 * The popup's own chrome is the other half, and only a browser can check it.
 * `.image-popup-content` clips its children, so a control positioned against
 * that box and sitting outside it is not drawn. The close button was pinned
 * above a box that grew to a fixed 95vh, which put it off the top of the
 * screen. Clicking it here is the assertion: Playwright's hit test fails on a
 * control that is in the DOM but not reachable, which a visibility check alone
 * would pass.
 *
 * The zoom level is the third half, and it is a measurement of the rendered
 * image, so nothing but a browser can check it. An image smaller than the
 * window used to open at its own size. The control read out 100% there, which
 * named neither the level nor the fit. Both images below are opened for that:
 * one the window has to shrink, one it has to blow up.
 *
 * Desktop-only. The mobile popup keeps its own floating close button, and pinch
 * is covered by the pure gesture math in src/utils/pinchGesture.test.ts.
 */
import { test, expect, type Page } from './fixtures';
import {
  assertHealthy,
  ensureOnThreadPane,
  waitForVisibleInput,
  openFilesPanel,
  waitForVisibleElement,
  clickVisibleElement,
  gotoWithRetry,
} from './helpers';

test.use({ viewport: { width: 1280, height: 800 } });

// Tall and narrow, like the screenshot that prompted this: fitted to the pane
// it is a thin strip, so every zoom step is plainly visible. The window has to
// shrink it to fit, so the fitted view sits well under 1:1.
const TALL_IMAGE = `<svg xmlns="http://www.w3.org/2000/svg" width="240" height="2000">
  <rect width="240" height="2000" fill="#123456"/>
  <rect x="20" y="20" width="200" height="180" fill="#ffcc00"/>
</svg>
`;

// Smaller than any window it opens in, so fitting it means blowing it up.
const SMALL_IMAGE = `<svg xmlns="http://www.w3.org/2000/svg" width="100" height="80">
  <rect width="100" height="80" fill="#123456"/>
</svg>
`;

let tallName: string;
let smallName: string;
let paths: string[] = [];

/** The scale the popup has applied to the image on screen. */
async function popupScale(page: Page): Promise<number> {
  return page.locator('.image-popup-slide img').first().evaluate((el) => {
    const match = (el as HTMLElement).style.transform.match(/scale\(([\d.]+)\)/);
    return match ? parseFloat(match[1]) : 1;
  });
}

/** The percentage the level control reads out, as a number. */
async function readoutPercent(page: Page): Promise<number> {
  const text = await page.locator('.image-popup-zoom-level').innerText();
  return parseInt(text, 10);
}

/** Open the Files pane and click one file through to its image popup. */
async function openPopup(page: Page, name: string): Promise<void> {
  await openFilesPanel(page);
  await waitForVisibleElement(page, '.file-item', 15_000);
  expect(await clickVisibleElement(page, '.file-item', name)).toBe(true);

  const preview = page.locator('.preview-image:visible').first();
  await expect(preview).toBeVisible({ timeout: 10_000 });
  await expect(preview).toHaveCSS('cursor', 'zoom-in');
  await preview.click();
  await expect(page.locator('.image-popup-content')).toBeVisible({ timeout: 5_000 });
}

test.describe('a previewed image opens full size', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    const stamp = Date.now();
    tallName = `e2e-preview-image-tall-${stamp}.svg`;
    smallName = `e2e-preview-image-small-${stamp}.svg`;
    paths = [`artifacts/${tallName}`, `artifacts/${smallName}`];
    for (const [path, body] of [[paths[0], TALL_IMAGE], [paths[1], SMALL_IMAGE]] as const) {
      const resp = await page.request.put(`/api/v1/data/${path}`, {
        headers: { 'Content-Type': 'image/svg+xml' },
        data: body,
      });
      expect(resp.ok()).toBeTruthy();
    }

    await gotoWithRetry(page, '/');
    await page.waitForFunction(() =>
      document.querySelector('#app')?.childElementCount! > 0,
      undefined, { timeout: 30_000 },
    );
    await ensureOnThreadPane(page);
    await waitForVisibleInput(page);
  });

  test.afterEach(async ({ page }) => {
    for (const path of paths) await page.request.delete(`/api/v1/data/${path}`);
    paths = [];
  });

  test('click opens the popup, whose controls zoom and close it', async ({ page }) => {
    await openPopup(page, tallName);
    const popup = page.locator('.image-popup-content');
    const level = page.locator('.image-popup-zoom-level');

    // The window shrinks this one to fit, so the fitted view is well under 1:1
    // and the readout says so rather than claiming 100%.
    expect(await popupScale(page)).toBe(1);
    await expect.poll(() => readoutPercent(page), { timeout: 5_000 }).toBeLessThan(100);
    // Fitted IS zoomed all the way out here, so there is nowhere further to go.
    await expect(page.locator('.image-popup-zoom-btn[aria-label="Zoom out"]')).toBeDisabled();

    // Zoom in one step. The scale is applied on the next frame, so poll.
    await page.locator('.image-popup-zoom-btn[aria-label="Zoom in"]').click();
    await expect.poll(() => popupScale(page), { timeout: 5_000 }).toBeGreaterThan(1);

    // Zoom out returns to the fitted view, the floor of the range.
    await page.locator('.image-popup-zoom-btn[aria-label="Zoom out"]').click();
    await expect.poll(() => popupScale(page), { timeout: 5_000 }).toBe(1);

    // The level control toggles the fitted view against 1:1, and the readout
    // follows the image rather than naming the button's next action.
    await level.click();
    // 240 CSS px wide fitted to a ~760px tall pane, so 1:1 is a long way up.
    await expect.poll(() => popupScale(page), { timeout: 5_000 }).toBeGreaterThan(2);
    await expect(level).toHaveText('100%');
    await level.click();
    await expect.poll(() => popupScale(page), { timeout: 5_000 }).toBe(1);

    // The close button is reachable, not merely present: a control clipped by
    // the popup's own box fails this click rather than this assertion.
    await page.locator('.image-popup-close').click();
    await expect(popup).toHaveCount(0);
  });

  test('an image smaller than the window opens filling it, not at its own size', async ({ page }) => {
    await openPopup(page, smallName);
    const level = page.locator('.image-popup-zoom-level');

    // 100x80 in a 1216x760 box: fitting it means blowing it up ~9.5x.
    await expect.poll(() => popupScale(page), { timeout: 5_000 }).toBeGreaterThan(5);
    await expect.poll(() => readoutPercent(page), { timeout: 5_000 }).toBeGreaterThan(100);

    // Its own pixels are still reachable, and are the floor of the range.
    await level.click();
    await expect.poll(() => popupScale(page), { timeout: 5_000 }).toBeLessThan(1.01);
    await expect(level).toHaveText('100%');
    await expect(page.locator('.image-popup-zoom-btn[aria-label="Zoom out"]')).toBeDisabled();

    // And the same control puts it back where it opened.
    await level.click();
    await expect.poll(() => popupScale(page), { timeout: 5_000 }).toBeGreaterThan(5);
  });
});
