import type { Locator } from '@playwright/test';
import { test, expect } from './fixtures';
import { navigateToApp, assertHealthy, waitForVisibleInput } from './helpers';

// On a phone, a dropdown opened mid-message left the keyboard on the prompt.
// Every keystroke then went into the message instead of filtering the menu.
// A touch menu always shows its filter box, and one opened with a text field
// focused also focuses it (`keyboardHandoff.ts`). Closing hands focus back.
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

    // The first Escape steps back by clearing the filter; the second closes.
    await page.keyboard.press('Escape');
    await expect(filter).toHaveValue('');
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
    const filter = page.locator('.dropdown-menu .dropdown-filter');
    await expect(filter).toBeVisible();
    await expect(filter).not.toBeFocused();
    // Any text field raises the keyboard, as `opensSoftwareKeyboard` counts them.
    const focusTakesKeyboard = await page.evaluate(() => {
      const el = document.activeElement;
      if (!(el instanceof HTMLElement)) return false;
      return el.isContentEditable || el.tagName === 'INPUT' || el.tagName === 'TEXTAREA';
    });
    expect(focusTakesKeyboard).toBe(false);
  });
});
