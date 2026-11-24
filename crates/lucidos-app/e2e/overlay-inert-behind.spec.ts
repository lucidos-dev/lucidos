import { test, expect } from './fixtures';
import { navigateToApp, assertHealthy } from './helpers';

test.describe('Overlay makes the UI behind it inert', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('opening a backdrop-less popover disables hover/clicks on the UI behind it but keeps the anchor live', async ({ page }) => {
    await navigateToApp(page);

    const behind = page.locator('.thread-toggle:visible').first();
    const pe = (loc: typeof behind) => loc.evaluate((el) => getComputedStyle(el).pointerEvents);
    expect(await pe(behind)).not.toBe('none');

    const anchor = page.locator('.compose-destination-picker .dropdown-trigger:visible').first();
    await anchor.click();
    const menu = page.locator('.dropdown-menu:visible').first();
    await expect(menu).toBeVisible();

    const overlayOpen = () => page.evaluate(() => document.documentElement.hasAttribute('data-overlay-open'));
    await expect.poll(overlayOpen).toBe(true);
    await expect.poll(() => pe(behind)).toBe('none');
    await expect.poll(() => pe(menu)).toBe('auto');
    await expect.poll(() => pe(anchor)).toBe('auto');
    const appShellPe = await page.evaluate(
      () => getComputedStyle(document.querySelector('.app-shell')!).pointerEvents,
    );
    expect(appShellPe).not.toBe('none');

    await page.keyboard.press('Escape');
    await expect(page.locator('.dropdown-menu')).toHaveCount(0);
    await expect.poll(overlayOpen).toBe(false);
    await expect.poll(() => pe(behind)).not.toBe('none');
  });
});
