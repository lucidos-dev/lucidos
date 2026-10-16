import { test, expect } from './fixtures';
import {
  apiRequest, assertHealthy, clickVisibleElement, navigateToApp, newThread,
  pickComposeDestination,
} from './helpers';
import { clearAllThreads } from './db-helpers';

/** The composer's command menu lays over the header when the viewport is short.
 *
 *  With the iOS keyboard up, the space above the composer is shorter than the
 *  menu. Its head, the filter box and first rows, must paint above the header
 *  rather than under it. A short viewport stands in for the keyboard.
 *
 *  The check is a hit test at a point inside both the panel's head and the
 *  header. An open overlay makes the header click-through, and a hit test
 *  skips click-through elements, so the probe lifts that for its one read.
 *  Otherwise it would see through the header and measure nothing. */

/** Short enough that the menu's head reaches up into the header. */
const KEYBOARD_UP_HEIGHT = 380;

test.describe('the composer command menu on a short phone viewport', () => {
  test.beforeEach(async ({ page }) => {
    clearAllThreads();
    await assertHealthy(page);
    const prefReset = await apiRequest(page).put('/api/v1/preferences?key=coding_agent_default', {
      data: { value: 'claude-code' },
    });
    expect(prefReset.ok()).toBeTruthy();
  });

  test('paints its head over the header', async ({ page }) => {
    const restore = page.viewportSize();
    await page.setViewportSize({ width: restore?.width ?? 390, height: KEYBOARD_UP_HEIGHT });
    await navigateToApp(page);
    await newThread(page);
    await pickComposeDestination(page);

    // The destination pick reloads the commands, and the button can read ready
    // on the old list and then drop it. A tap then opens nothing, so tap until
    // the menu is up rather than trusting one ready check.
    const panel = page.locator('.control-dropdown:visible');
    await expect(async () => {
      if (await panel.count() === 0) {
        await clickVisibleElement(page, '.commands-btn-active:not(.lucidos-commands-btn)');
      }
      await expect(panel).toHaveCount(1, { timeout: 2_000 });
    }).toPass({ timeout: 20_000 });

    // Placement lands a frame after mount, so poll until the head reads clear.
    await expect.poll(() => panel.evaluate((el) => {
      const rect = el.getBoundingClientRect();
      if (rect.top < 0) return 'head above the viewport';
      const header = document.querySelector('.app-header')!.getBoundingClientRect();
      const y = rect.top + 4;
      if (y >= header.bottom) return 'head below the header, so nothing was tested';
      const lift = document.createElement('style');
      lift.textContent = '* { pointer-events: auto !important; }';
      document.head.append(lift);
      const hit = document.elementFromPoint(rect.left + rect.width / 2, y);
      lift.remove();
      return hit && el.contains(hit) ? 'clear' : `covered by ${hit?.className ?? 'nothing'}`;
    })).toBe('clear');

    // The filter box is the panel's first row, the one the header hid.
    await expect(page.locator('.control-dropdown .control-filter')).toBeInViewport();

    await page.keyboard.press('Escape');
    await page.setViewportSize({ width: restore?.width ?? 390, height: restore?.height ?? 844 });
    await assertHealthy(page);
  });
});
