import { test, expect } from './fixtures';
import { assertHealthy, navigateToApp, openSettingsView } from './helpers';

const CARD_ANCHOR = '[data-search-anchor="providers:chatgpt-plan"]:visible';

test.describe('ChatGPT plan card', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('signed out, it offers Continue with ChatGPT and toasts a refused start', async ({ page, context }) => {
    await page.route('**/api/v1/health', async (route) => {
      const response = await route.fetch();
      const health = await response.json();
      await route.fulfill({ response, json: { ...health, configured_providers: [] } });
    });
    await navigateToApp(page);
    await openSettingsView(page, 'providers');
    await expect(page.locator(CARD_ANCHOR)).toBeVisible({ timeout: 15_000 });

    const toggle = page.getByRole('checkbox', { name: 'Enable ChatGPT plan' });
    await expect(toggle).not.toBeChecked();
    await page.locator('label.toggle-switch', { has: toggle }).click();

    const button = page.locator('[data-oai-siwc-button]:visible');
    await expect(button).toHaveText('Continue with ChatGPT');
    await expect(button).toBeEnabled();

    await page.route('**/api/v1/providers/chatgpt-plan/sign-in', (route) =>
      route.continue({
        headers: { ...route.request().headers(), 'x-forwarded-prefix': '/e2e-test/' },
      }),
    );
    let opened = false;
    context.on('page', () => { opened = true; });

    await button.click();
    const toast = page.locator('.toast-error:visible').first();
    await expect(toast).toContainText('Could not start Sign in with ChatGPT');
    await expect(button).toHaveText('Continue with ChatGPT');
    await expect(button).toBeEnabled();
    expect(opened, 'a refused start opens no ChatGPT page').toBe(false);
  });
});
