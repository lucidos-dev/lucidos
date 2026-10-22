import type { Locator } from '@playwright/test';
import { test, expect } from './fixtures';
import { navigateToApp, assertHealthy, waitForVisibleInput } from './helpers';

// Opened under a finger mid-message, the rung list's filter box takes the
// on-screen keyboard straight from the prompt, and closing hands it back
// (`keyboardHandoff.ts`). The keyboard never drops. The Lucidos Agent's list
// stands in for both agent kinds: the coding-agent list is the same picker,
// and the unit tests cover it.
test.describe('A touch rung list keeps the on-screen keyboard up', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    await navigateToApp(page);
  });

  const isFocused = (loc: Locator) => loc.evaluate((el) => el === document.activeElement);

  test('opened mid-message, the filter takes the keyboard, and Escape hands it back with the text', async ({ page }) => {
    const prompt = await waitForVisibleInput(page);
    await prompt.click();
    await prompt.fill('draft');

    await page.locator('.rung-trigger:visible').first().click();
    await expect(page.locator('.control-dropdown .control-option').first()).toBeVisible();
    const filter = page.locator('.control-dropdown .control-filter');
    await expect.poll(() => isFocused(filter)).toBe(true);

    await page.keyboard.press('Escape');
    await expect(page.locator('.control-dropdown')).toHaveCount(0);
    await expect(prompt).toHaveValue('draft');
    await expect.poll(() => isFocused(prompt)).toBe(true);
  });

  test('a list opened with no field focused raises no keyboard', async ({ page }) => {
    await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());

    await page.locator('.rung-trigger:visible').first().click();
    await expect(page.locator('.control-dropdown .control-option').first()).toBeVisible();
    expect(await page.evaluate(() => document.activeElement === document.body)).toBe(true);
  });
});
