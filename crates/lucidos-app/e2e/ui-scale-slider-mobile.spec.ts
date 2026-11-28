import { test, expect, Page } from './fixtures';
import { apiRequest, assertHealthy, isMobileViewport, navigateToApp, waitForEventStream, waitForPaneAtRest } from './helpers';
import { psql } from './db-helpers';

const UI_SCALE_BUTTON = '[data-search-anchor="appearance:ui-scale"] .settings-option';

async function openScalePanel(page: Page): Promise<void> {
  await navigateToApp(page);
  await waitForEventStream(page);

  const res = await apiRequest(page).post('/api/v1/ui/navigate', {
    headers: { 'content-type': 'application/json' },
    data: { target: 'settings', params: { settings_view: 'appearance' } },
  });
  expect(res.ok(), `POST /api/v1/ui/navigate -> ${res.status()}`).toBeTruthy();

  const button = page.locator(UI_SCALE_BUTTON);
  await expect(button).toBeVisible({ timeout: 15_000 });
  await button.scrollIntoViewIfNeeded();
  await waitForPaneAtRest(page);
  await button.tap();
  await expect(page.locator('.scale-modal')).toBeVisible();
}

async function appliedScale(page: Page): Promise<string> {
  return page.evaluate(() =>
    document.documentElement.style.getPropertyValue('--user-ui-scale').trim());
}

async function tapRowAt(page: Page, fraction: number): Promise<void> {
  const box = await page.locator('.scale-modal-slider').boundingBox();
  expect(box, 'the slider row has no box').not.toBeNull();
  await page.touchscreen.tap(box!.x + box!.width * fraction, box!.y + box!.height / 2);
}

test.describe('Mobile UI-scale slider', () => {
  let savedScale = false;

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    test.skip(!isMobileViewport(page), 'Touch behavior only, so the desktop project skips it');
    savedScale = false;
    page.on('request', (request) => {
      const url = new URL(request.url());
      if (request.method() === 'PUT' && url.pathname.endsWith('/api/v1/preferences')
        && url.searchParams.get('key') === 'ui-scale') {
        savedScale = true;
      }
    });
  });

  test.afterEach(async ({ page }) => {
    if (savedScale) {
      const last = (await appliedScale(page)).replace(/%$/, '');
      await expect.poll(
        () => psql(`SELECT count(*) FROM preferences WHERE key = 'ui-scale' AND value = '${last}'`),
        { message: `the last ui-scale save (${last}) never committed`, timeout: 10_000 },
      ).not.toBe('0');
    }
    psql(`DELETE FROM preferences WHERE key = 'ui-scale'`);
  });

  test('a touch away from the thumb moves the scale to where the finger is', async ({ page }) => {
    await openScalePanel(page);

    await tapRowAt(page, 0.98);
    await expect(page.locator('.scale-modal-label')).toHaveText('200%');
    expect(await appliedScale(page)).toBe('200%');

    await tapRowAt(page, 0.02);
    await expect(page.locator('.scale-modal-label')).toHaveText('75%');
    expect(await appliedScale(page)).toBe('75%');
  });

  test('a drag steers the scale continuously and holds where it is released', async ({ page }) => {
    await openScalePanel(page);
    const box = (await page.locator('.scale-modal-slider').boundingBox())!;
    const y = box.y + box.height / 2;

    await page.mouse.move(box.x + box.width * 0.05, y);
    await page.mouse.down();
    await page.mouse.move(box.x + box.width * 0.98, y, { steps: 12 });
    await expect(page.locator('.scale-modal-label')).toHaveText('200%');

    await page.mouse.move(box.x - 60, y + 200, { steps: 12 });
    await expect(page.locator('.scale-modal-label')).toHaveText('75%');

    await page.mouse.up();
    await expect(page.locator('.scale-modal')).toBeVisible();
    expect(await appliedScale(page)).toBe('75%');
  });

  test('arrow keys step the scale after a press has focused the row', async ({ page }) => {
    await openScalePanel(page);
    await tapRowAt(page, 0.02);
    await expect(page.locator('.scale-modal-label')).toHaveText('75%');

    await page.keyboard.press('ArrowRight');
    await expect(page.locator('.scale-modal-label')).toHaveText('87.5%');

    await page.keyboard.press('End');
    await expect(page.locator('.scale-modal-label')).toHaveText('200%');
    expect(await appliedScale(page)).toBe('200%');
  });

  test('the thumb is sized to be read, not to be caught', async ({ page }) => {
    await openScalePanel(page);
    const thumb = (await page.locator('.scale-modal-thumb').boundingBox())!;

    expect(thumb.width).toBeGreaterThan(12);
    expect(thumb.width).toBeLessThanOrEqual(24);
    expect(thumb.height).toBeCloseTo(thumb.width, 0);

    const row = (await page.locator('.scale-modal-slider').boundingBox())!;
    expect(row.height).toBeGreaterThanOrEqual(40);
  });
});
