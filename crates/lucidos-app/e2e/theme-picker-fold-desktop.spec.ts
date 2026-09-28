/**
 * Settings > Appearance > Theme folds back to the active theme's card on any
 * click that picks no theme, with a real mouse. Unit tests dispatch synthetic
 * clicks on `document.body`, which says nothing about a mouse click landing on
 * the settings page.
 */
import { test, expect } from './fixtures';
import { apiRequest, assertHealthy, navigateToApp, waitForEventStream } from './helpers';

test.use({ viewport: { width: 1280, height: 900 } });

test('the theme gallery folds on any click that picks no theme', async ({ page }) => {
  await assertHealthy(page);
  // Dark, so a light-only theme raises the mode-switch confirm below.
  await page.emulateMedia({ colorScheme: 'dark' });
  await navigateToApp(page);
  await waitForEventStream(page);
  const nav = await apiRequest(page).post('/api/v1/ui/navigate', {
    headers: { 'content-type': 'application/json' },
    data: { target: 'settings', params: { settings_view: 'appearance' } },
  });
  expect(nav.ok(), `POST /api/v1/ui/navigate -> ${nav.status()}`).toBeTruthy();

  const radios = page.locator('.theme-card[role="radio"]');
  const elsewhere = page.locator('.settings-section-title', { hasText: 'Typography' });

  // With no pick at all, a click elsewhere folds it.
  await page.locator('.theme-toggle').click();
  await expect(radios.first()).toBeVisible();
  await elsewhere.click();
  await expect(radios).toHaveCount(0);

  // A pick keeps it open for comparing, and the next click elsewhere folds it.
  // A theme for both modes picks at once, with no confirm.
  await page.locator('.theme-toggle').click();
  await expect(radios.first()).toBeVisible();
  const other = page.locator('.theme-card[role="radio"][aria-checked="false"]:not(:has(.theme-card-modes))').first();
  const name = await other.locator('.theme-card-name').innerText();
  await other.click();
  await expect(page.locator('.theme-card[role="radio"][aria-checked="true"]')).toContainText(name);

  await expect(radios.first()).toBeVisible();
  await elsewhere.click();
  await expect(radios).toHaveCount(0);
  await expect(page.locator('.theme-toggle')).toContainText(name);

  // A light-only theme picked in dark asks first. Confirming picks it, so the
  // gallery stays open.
  await page.locator('.theme-toggle').click();
  await expect(page.locator('html')).toHaveAttribute('data-theme-mode', 'dark');
  await radios.filter({ hasText: 'Light only' }).first().click();
  await page.getByRole('button', { name: 'Switch to light mode' }).click();
  await expect(radios.first()).toBeVisible();
  await elsewhere.click();
  await expect(radios).toHaveCount(0);
});
