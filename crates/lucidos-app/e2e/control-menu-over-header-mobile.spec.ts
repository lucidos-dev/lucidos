import { test, expect } from './fixtures';
import {
  apiRequest, assertHealthy, composerControl, navigateToApp, newThread,
  pickComposeDestination, ensureSlashCommandsReported,
} from './helpers';
import { clearAllThreads } from './db-helpers';


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
    await ensureSlashCommandsReported(page);
    const restore = page.viewportSize();
    await page.setViewportSize({ width: restore?.width ?? 390, height: KEYBOARD_UP_HEIGHT });
    await navigateToApp(page);
    await newThread(page);
    await pickComposeDestination(page);

    const panel = page.locator('.control-dropdown:visible');
    await expect(async () => {
      if (await panel.count() === 0) {
        await (await composerControl(page, '.commands-btn')).click();
      }
      await expect(panel).toHaveCount(1, { timeout: 2_000 });
    }).toPass({ timeout: 20_000 });

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

    await expect(page.locator('.control-dropdown .control-filter')).toBeInViewport();

    await page.keyboard.press('Escape');
    await page.setViewportSize({ width: restore?.width ?? 390, height: restore?.height ?? 844 });
    await assertHealthy(page);
  });
});
