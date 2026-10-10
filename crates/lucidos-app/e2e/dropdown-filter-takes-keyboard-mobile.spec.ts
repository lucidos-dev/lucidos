import type { Locator } from '@playwright/test';
import { test, expect } from './fixtures';
import { navigateToApp, assertHealthy, waitForVisibleInput } from './helpers';

// On a phone, a dropdown opened mid-message left the keyboard on the prompt.
// Every keystroke then went into the message instead of filtering the menu.
// A touch menu opened with a text field focused now shows its filter box and
// focuses it (`keyboardHandoff.ts`), and closing hands focus back.
test.describe('A touch dropdown takes over the on-screen keyboard', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    await navigateToApp(page);
  });

  const isFocused = (loc: Locator) => loc.evaluate((el) => el === document.activeElement);

  test('typing filters the menu, not the prompt, and Escape hands focus back', async ({ page }) => {
    const prompt = await waitForVisibleInput(page);
    await prompt.click();
    await prompt.fill('draft');

    await page.locator('.compose-destination-picker .dropdown-trigger:visible').first().click();
    const filter = page.locator('.dropdown-menu .dropdown-filter');
    await expect.poll(() => isFocused(filter)).toBe(true);

    await page.keyboard.type('zzz-no-such-target');
    await expect(filter).toHaveValue('zzz-no-such-target');
    await expect(page.locator('.dropdown-menu .dropdown-no-results')).toBeVisible();
    await expect(prompt).toHaveValue('draft');

    await page.keyboard.press('Escape');
    await expect(page.locator('.dropdown-menu')).toHaveCount(0);
    await expect.poll(() => isFocused(prompt)).toBe(true);
  });

  test('picking an option hands focus back to the prompt', async ({ page }) => {
    const prompt = await waitForVisibleInput(page);
    await prompt.click();

    await page.locator('.compose-destination-picker .dropdown-trigger:visible').first().click();
    await expect.poll(() => isFocused(page.locator('.dropdown-menu .dropdown-filter'))).toBe(true);

    await page.locator('.dropdown-menu .dropdown-option:not(.dropdown-option-header)').first().click();
    await expect(page.locator('.dropdown-menu')).toHaveCount(0);
    await expect.poll(() => isFocused(prompt)).toBe(true);
  });

  test('a menu opened with no field focused raises no keyboard', async ({ page }) => {
    await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());

    await page.locator('.compose-destination-picker .dropdown-trigger:visible').first().click();
    await expect(page.locator('.dropdown-menu:visible')).toBeVisible();
    await expect(page.locator('.dropdown-menu .dropdown-filter')).toHaveCount(0);
  });
});
