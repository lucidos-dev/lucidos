import { test, expect, type Locator } from './fixtures';
import { navigateToApp, sendMessage, waitForResponse, assertHealthy } from './helpers';

async function scrollSettled(area: Locator): Promise<void> {
  await area.evaluate((el) => new Promise<void>((resolve) => {
    let last = el.scrollTop;
    let still = 0;
    const tick = () => {
      if (el.scrollTop === last) still += 1;
      else { still = 0; last = el.scrollTop; }
      if (still >= 3) resolve();
      else requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
  }));
}

test.describe('Keyboard scrolling of the conversation (desktop)', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('transcript is focusable + keyboard-scrollable, Space is not stolen, and ⌘↑ traverses turns', async ({ page }) => {

    await page.setViewportSize({ width: 1280, height: 300 });
    await navigateToApp(page);

    await sendMessage(page, 'List the numbers from 1 to 40, one per line, and nothing else.');
    await waitForResponse(page);

    const tc = page.locator('.thread-content.visible:visible').first();
    await expect(tc).toHaveAttribute('tabindex', '0');
    await expect(tc).toHaveAttribute('role', 'region');

    await tc.evaluate((el) => (el as HTMLElement).focus());
    await scrollSettled(tc);
    await page.keyboard.press('Home');
    await expect.poll(() => tc.evaluate((el) => el.scrollTop)).toBeLessThan(2);
    await scrollSettled(tc);
    await page.keyboard.press('End');
    await expect.poll(() => tc.evaluate((el) => el.scrollTop)).toBeGreaterThan(2);

    await scrollSettled(tc);
    await page.keyboard.press('Home');
    await expect.poll(() => tc.evaluate((el) => el.scrollTop)).toBeLessThan(2);
    await scrollSettled(tc);
    await page.keyboard.press('Space');
    expect(
      await page.evaluate(() => document.activeElement?.classList.contains('thread-content') ?? false),
      'Space moved focus off the transcript — it was stolen by type-to-focus',
    ).toBe(true);
    await expect.poll(() => tc.evaluate((el) => el.scrollTop)).toBeGreaterThan(2);

    await page.locator('[data-role="prompt-input"]:visible').first().focus();
    await page.keyboard.press('ControlOrMeta+ArrowUp');
    await expect.poll(() =>
      page.evaluate(() => document.activeElement?.classList.contains('thread-content') ?? false),
    ).toBe(true);
  });
});
