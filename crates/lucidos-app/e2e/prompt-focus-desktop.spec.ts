import { test, expect } from './fixtures';
import { navigateToApp, assertHealthy, waitForVisibleInput, waitForResponse } from './helpers';

test.describe('Composer focus (desktop)', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('sending with Enter leaves the caret in the composer', async ({ page }) => {
    await navigateToApp(page);
    const input = await waitForVisibleInput(page);

    await input.fill('Hello from the composer focus spec');
    await input.press('Enter');
    await waitForResponse(page);

    await expect.poll(
      () => page.evaluate(() =>
        (document.activeElement as HTMLElement | null)?.dataset?.role ?? null),
      { intervals: [200], timeout: 10_000 },
    ).toBe('prompt-input');
  });

  test('clicking the thread pane header puts the caret in the composer', async ({ page }) => {
    await navigateToApp(page);
    const input = await waitForVisibleInput(page);

    await input.evaluate((el: HTMLTextAreaElement) => el.blur());
    await expect.poll(() => page.evaluate(() =>
      (document.activeElement as HTMLElement | null)?.dataset?.role ?? null)).not.toBe('prompt-input');

    await page.locator('.desktop-header .pane-header-brand:visible').first()
      .dispatchEvent('click');

    await expect.poll(
      () => page.evaluate(() =>
        (document.activeElement as HTMLElement | null)?.dataset?.role ?? null),
      { intervals: [200], timeout: 5_000 },
    ).toBe('prompt-input');
  });
});
