import type { Locator } from '@playwright/test';
import { test, expect } from './fixtures';
import { navigateToApp, assertHealthy, waitForVisibleInput } from './helpers';

// The rung list has no field to type in, so opening it under a finger never
// takes the on-screen keyboard from the prompt (`keyboardHandoff.ts`). The
// Lucidos Agent's list stands in for both agent kinds: the coding-agent list
// is the same picker, and the unit tests cover it.
test.describe('A touch rung list keeps the on-screen keyboard with the prompt', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    await navigateToApp(page);
  });

  const isFocused = (loc: Locator) => loc.evaluate((el) => el === document.activeElement);

  test('opened mid-message, the prompt keeps focus and its text, and Escape closes the list', async ({ page }) => {
    const prompt = await waitForVisibleInput(page);
    await prompt.click();
    await prompt.fill('draft');

    await page.locator('.rung-trigger:visible').first().click();
    await expect(page.locator('.control-dropdown .control-option').first()).toBeVisible();
    await expect(page.locator('.control-dropdown .control-filter')).toHaveCount(0);
    await expect.poll(() => isFocused(prompt)).toBe(true);

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
