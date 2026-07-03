import { test, expect } from './fixtures';
import { navigateToApp, sendMessage, waitForResponse, assertHealthy, isMobileViewport } from './helpers';

/** Keyboard scrolling of the conversation transcript. Two behaviors:
 *  1. `.thread-content` is a keyboard-focusable scroll region — once focused, the
 *     native Home/End/PageUp/PageDown/Arrow/Space keys scroll it.
 *  2. ⌘↑ / ⌘↓ traverse turn-by-turn and land focus in the transcript so
 *     continuous scrolling follows — even when fired from the prompt.
 *  Both are desktop-only (mobile navigates panes and has no physical chord path). */
test.describe('Keyboard scrolling of the conversation (desktop)', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('transcript is focusable + keyboard-scrollable, Space is not stolen, and ⌘↑ traverses turns', async ({ page }) => {
    test.skip(isMobileViewport(page), 'keyboard scroll + turn nav are desktop-only');

    // A short viewport so even a modest transcript overflows → deterministic
    // scroll-position assertions.
    await page.setViewportSize({ width: 1280, height: 300 });
    await navigateToApp(page);

    // A long-ish reply guarantees the transcript overflows the short viewport.
    await sendMessage(page, 'List the numbers from 1 to 40, one per line, and nothing else.');
    await waitForResponse(page);

    const tc = page.locator('.thread-content.visible:visible').first();
    // The scroll container is a keyboard-focusable region.
    await expect(tc).toHaveAttribute('tabindex', '0');
    await expect(tc).toHaveAttribute('role', 'region');

    // Focus it and confirm native keyboard scrolling: Home → top, End → down.
    await tc.evaluate((el) => (el as HTMLElement).focus());
    await page.keyboard.press('Home');
    await expect.poll(() => tc.evaluate((el) => el.scrollTop)).toBeLessThan(2);
    await page.keyboard.press('End');
    await expect.poll(() => tc.evaluate((el) => el.scrollTop)).toBeGreaterThan(2);

    // Space with the transcript focused pages it down — it must NOT be captured by
    // type-to-focus (focus stays on the transcript, and it scrolled).
    await page.keyboard.press('Home');
    await expect.poll(() => tc.evaluate((el) => el.scrollTop)).toBeLessThan(2);
    await page.keyboard.press('Space');
    expect(
      await page.evaluate(() => document.activeElement?.classList.contains('thread-content') ?? false),
      'Space moved focus off the transcript — it was stolen by type-to-focus',
    ).toBe(true);
    await expect.poll(() => tc.evaluate((el) => el.scrollTop)).toBeGreaterThan(2);

    // ⌘↑ (ControlOrMeta for cross-platform) traverses a turn and lands focus in the
    // transcript so continuous scrolling follows — even when fired from the prompt.
    await page.locator('[data-role="prompt-input"]:visible').first().focus();
    await page.keyboard.press('ControlOrMeta+ArrowUp');
    await expect.poll(() =>
      page.evaluate(() => document.activeElement?.classList.contains('thread-content') ?? false),
    ).toBe(true);
  });
});
