/**
 * Desktop: a ctrl/meta wheel zoom spends DISTANCE, and a burst of it cannot
 * wedge the tab.
 *
 * Regression. The panel took every ctrl/meta wheel event as one full 12.5%
 * step. A mouse is one event per notch, so a mouse read correctly. A trackpad
 * pinch is a stream of small-delta events, so one gesture asked for dozens of
 * steps. Each step applied at once and forced a full layout at a new root font
 * size. On a long transcript that ran the main thread out of frames.
 *
 * `docs/plans/2026-09-19-zooming-cannot-wedge-the-tab.md` has the full account.
 */

/* The bursts are dispatched inside the page, not through `page.mouse.wheel`.
 * The accumulator ends a gesture after a quiet gap, and forty awaited round
 * trips would cross it. One real notch goes through the mouse as well, so the
 * wiring is proven end to end.
 *
 * Responsiveness is asserted by the test finishing at all. A wedged main thread
 * answers no query, so the assertion after each burst is the check. */
import { test, expect, Page } from './fixtures';
import { apiRequest, assertHealthy, isMobileViewport, navigateToApp, waitForEventStream } from './helpers';

const UI_SCALE_BUTTON = '[data-search-anchor="appearance:ui-scale"] .settings-option';

/** Land on Settings > Appearance and open the scale panel from its row.
 *  Opened this way it has no linger countdown, so it stays up between bursts. */
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
  await button.click();
  await expect(page.locator('.scale-modal')).toBeVisible();
  await expect(page.locator('.scale-modal-label')).toHaveText('100%');
}

/** Dispatch `count` ctrl+wheel events of `deltaY`, with no gap between them. */
async function pinch(page: Page, count: number, deltaY: number): Promise<void> {
  await page.evaluate(({ count, deltaY }) => {
    for (let i = 0; i < count; i++) {
      document.dispatchEvent(new WheelEvent('wheel', {
        deltaY, ctrlKey: true, bubbles: true, cancelable: true,
      }));
    }
  }, { count, deltaY });
}

/** The scale `<html>` is actually rendering at. */
async function appliedScale(page: Page): Promise<string> {
  return page.evaluate(() =>
    document.documentElement.style.getPropertyValue('--user-ui-scale').trim());
}

test.describe('Desktop UI-scale wheel zoom', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    test.skip(isMobileViewport(page), 'A wheel is a desktop input');
  });

  test('a pinch shorter than one notch moves nothing', async ({ page }) => {
    await openScalePanel(page);

    // Twenty events carrying 4px each. Twenty steps, before the fix.
    await pinch(page, 20, -4);
    await page.waitForTimeout(200);
    await expect(page.locator('.scale-modal-label')).toHaveText('100%');
  });

  test('a pinch moves by the distance it travelled, not by its event count', async ({ page }) => {
    await openScalePanel(page);

    // 40 events of 8px is 320px, which is three notches and a remainder.
    await pinch(page, 40, -8);
    await expect(page.locator('.scale-modal-label')).toHaveText('137.5%');
    expect(await appliedScale(page)).toBe('137.5%');
  });

  test('a long burst drains instead of slamming to the clamp', async ({ page }) => {
    await openScalePanel(page);

    // 400 events in one task. The old code applied one step per event, so this
    // is the shape that wedged the tab.
    await pinch(page, 400, -8);
    // The scale still lands where the distance asked, at the ceiling here, and
    // the page is answering, which is the point.
    await expect(page.locator('.scale-modal-label')).toHaveText('200%');

    // Reversing answers at once rather than paying off the old bank.
    await pinch(page, 13, 8);
    await expect(page.locator('.scale-modal-label')).toHaveText('187.5%');
  });

  test('one real wheel notch is one step', async ({ page }) => {
    await openScalePanel(page);

    await page.mouse.move(400, 400);
    await page.keyboard.down('Control');
    await page.mouse.wheel(0, -100);
    // Read the label while the modifier is still DOWN. Releasing it dismisses
    // the panel, which is the shortcut's own way out.
    await expect(page.locator('.scale-modal-label')).toHaveText('112.5%');

    await page.keyboard.up('Control');
    await expect(page.locator('.scale-modal')).toBeHidden();
    // The release keeps what the gesture asked for rather than reverting it.
    expect(await appliedScale(page)).toBe('112.5%');
  });
});
