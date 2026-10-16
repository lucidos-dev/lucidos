import type { Locator } from '@playwright/test';
import { test, expect } from './fixtures';
import { navigateToApp, assertHealthy, waitForVisibleInput } from './helpers';

// The prompt-bar agent menus take the keyboard the same way a dropdown does
// (`keyboardHandoff.ts`). Opened mid-message, the model filter takes focus, so
// typing filters the models instead of going into the prompt. Closing hands
// focus back. The Lucidos Agent menu stands in for all three: the coding-agent
// menu shares the helper and its unit tests.
test.describe('A touch agent menu takes over the on-screen keyboard', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    await navigateToApp(page);
  });

  const isFocused = (loc: Locator) => loc.evaluate((el) => el === document.activeElement);

  test('typing filters the models, not the prompt, and Escape hands focus back', async ({ page }) => {
    const prompt = await waitForVisibleInput(page);
    await prompt.click();
    await prompt.fill('draft');

    await page.locator('.lucidos-commands-btn:visible').first().click();
    const filter = page.locator('.control-dropdown .control-filter');
    await expect.poll(() => isFocused(filter)).toBe(true);
    await expect(page.locator('.control-dropdown .control-option').first()).toBeVisible();

    await page.keyboard.type('zzz-no-such-model');
    await expect(filter).toHaveValue('zzz-no-such-model');
    await expect(page.locator('.control-dropdown .control-option')).toHaveCount(0);
    await expect(prompt).toHaveValue('draft');

    await page.keyboard.press('Escape');
    await expect(page.locator('.control-dropdown')).toHaveCount(0);
    await expect.poll(() => isFocused(prompt)).toBe(true);
  });

  test('a menu opened with no field focused raises no keyboard', async ({ page }) => {
    await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());

    await page.locator('.lucidos-commands-btn:visible').first().click();
    await expect(page.locator('.control-dropdown .control-filter')).toBeVisible();
    expect(await page.evaluate(() => document.activeElement === document.body)).toBe(true);
  });
});
