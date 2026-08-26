/**
 * Mobile: the UI-scale slider answers a touch anywhere on its row, and its
 * thumb is back to a normal size.
 *
 * Regression. The slider was an `<input type="range">`, and WebKit starts a
 * range drag only on the thumb. A touch that landed beside it did nothing, so
 * resizing meant hitting a moving target. The thumb had been grown to 40px to
 * make it catchable, which is the "too big" half of the same report.
 *
 * ScaleModal now maps the pointer itself, so the whole row is the hit target
 * and the thumb is 20px. Both halves are asserted here, on the two mobile
 * projects: `mobile-webkit` is the engine the bug was reported on.
 *
 * The taps are real `touchscreen` taps rather than synthetic clicks, because a
 * click never exercised the failing path.
 */
import { test, expect, Page } from './fixtures';
import { assertHealthy, isMobileViewport, navigateToApp, waitForEventStream } from './helpers';

const UI_SCALE_BUTTON = '[data-search-anchor="appearance:ui-scale"] .settings-option';

/** Land on Settings > Appearance and open the scale panel from its row. */
async function openScalePanel(page: Page): Promise<void> {
  await navigateToApp(page);
  await waitForEventStream(page);

  const res = await page.request.post('/api/v1/ui/navigate', {
    headers: { 'content-type': 'application/json' },
    data: { target: 'settings', params: { settings_view: 'appearance' } },
  });
  expect(res.ok(), `POST /api/v1/ui/navigate -> ${res.status()}`).toBeTruthy();

  const button = page.locator(UI_SCALE_BUTTON);
  await expect(button).toBeVisible({ timeout: 15_000 });
  await button.tap();
  await expect(page.locator('.scale-modal')).toBeVisible();
}

/** The scale `<html>` is actually rendering at, which the label must agree with. */
async function appliedScale(page: Page): Promise<string> {
  return page.evaluate(() =>
    document.documentElement.style.getPropertyValue('--user-ui-scale').trim());
}

/** Tap the slider row `fraction` of the way along it. */
async function tapRowAt(page: Page, fraction: number): Promise<void> {
  const box = await page.locator('.scale-modal-slider').boundingBox();
  expect(box, 'the slider row has no box').not.toBeNull();
  await page.touchscreen.tap(box!.x + box!.width * fraction, box!.y + box!.height / 2);
}

test.describe('Mobile UI-scale slider', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    test.skip(!isMobileViewport(page), 'Touch behavior only, so the desktop project skips it');
  });

  /**
   * Both ends, for two reasons. They are the touches furthest from wherever the
   * thumb happens to be. They also land on the half-thumb of dead track, which
   * the mapping must clamp rather than read as out of range.
   */
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

    // Off the row entirely: pointer capture must keep the drag alive, and the
    // value must clamp rather than wander.
    await page.mouse.move(box.x - 60, y + 200, { steps: 12 });
    await expect(page.locator('.scale-modal-label')).toHaveText('75%');

    await page.mouse.up();
    await expect(page.locator('.scale-modal')).toBeVisible();
    expect(await appliedScale(page)).toBe('75%');
  });

  /**
   * The row carries `role="slider"`, which owes arrow keys. The range input it
   * replaced took them once a press had focused it. So a press focuses this row
   * too, and the keys pick up from there.
   */
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

    // The overlay pins a 16px root, so the 1.25em thumb is 20px. It was 40px when
    // the thumb had to be its own tap target.
    expect(thumb.width).toBeGreaterThan(12);
    expect(thumb.width).toBeLessThanOrEqual(24);
    expect(thumb.height).toBeCloseTo(thumb.width, 0);

    // The row it sits in stays a generous touch zone whatever the thumb does.
    const row = (await page.locator('.scale-modal-slider').boundingBox())!;
    expect(row.height).toBeGreaterThanOrEqual(40);
  });
});
