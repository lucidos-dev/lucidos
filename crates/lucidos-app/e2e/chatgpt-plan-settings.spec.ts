/**
 * Settings → Providers → ChatGPT plan, signed out (ADR 0636).
 *
 * The card opens on its switch and offers "Continue with ChatGPT". A start the
 * engine refuses arrives as an error toast naming Sign in with ChatGPT, and
 * opens no ChatGPT page. The sign-in itself runs against a mock auth server in
 * the engine's tests, and never spends plan usage.
 *
 * The e2e engine may run under a gateway, where a local browser's start would
 * succeed. So the test marks its sign-in request as proxied without the
 * gateway's local-browser stamp: the engine then refuses it in every mode.
 *
 * The mock model reports no provider list on `/health`, which keeps every
 * provider switch loading. The test reports an empty list instead: a
 * workspace with no provider installed, the plan's signed-out case.
 */
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
