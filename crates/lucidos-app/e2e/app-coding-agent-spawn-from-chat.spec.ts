import { test, expect } from './fixtures';
import { apiRequest, assertHealthy, navigateToApp } from './helpers';
import { createIframeAppFixture } from './db-helpers';

test.describe('App coding-agent thread — compose destination picker', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('destination picker exposes installed apps as coding targets', async ({ page }) => {
    const appId = `e2e-scope-${Date.now()}`;
    const fixture = createIframeAppFixture(appId, {
      html: '<!doctype html><title>scope picker fixture</title>',
      js: '/* noop */',
      manifest: { id: appId, name: `${appId} fixture`, description: 'scope picker e2e' },
    });
    try {
      const prefReset = await apiRequest(page).put('/api/v1/preferences?key=coding_agent_default', {
        data: { value: 'claude-code' },
      });
      expect(prefReset.ok()).toBeTruthy();

      await navigateToApp(page);

      const trigger = page.locator('.compose-destination-picker .dropdown-trigger:visible').first();
      await expect(trigger).toBeVisible({ timeout: 10_000 });
      await trigger.click();

      const menu = page.locator('.dropdown-menu:visible').first();
      await expect(menu).toBeVisible({ timeout: 5_000 });
      const codingHeader = menu.locator('.dropdown-option-header', { hasText: 'Coding agent on…' });
      await expect(codingHeader).toBeVisible();
      const appOption = menu.locator('.dropdown-option', { hasText: `${appId} fixture · app` });
      await expect(appOption).toBeVisible();
      await expect(menu.locator('.dropdown-option', { hasText: 'Lucidos Agent' }).first()).toBeVisible();
      await expect(menu.locator('.dropdown-option', { hasText: 'Lucidos source' }).first()).toBeVisible();

      await appOption.click();
      const stored = await page.evaluate(
        () => localStorage.getItem('lucidos-coding-agent-last-scope'),
      );
      expect(stored).toBeTruthy();
      const parsed = JSON.parse(stored!);
      expect(parsed.kind).toBe('app');
      expect(parsed.appId).toBe(appId);
      const mode = await page.evaluate(
        () => localStorage.getItem('lucidos-input-mode'),
      );
      expect(mode).toBeTruthy();
      expect(JSON.parse(mode!).type).toBe('coding_agent');

      await expect(
        page.locator('.compose-coding-agent-chip .dropdown-sizer > :first-child:visible').first(),
      ).toHaveText('Claude Code');
    } finally {
      fixture.cleanup();
    }
  });
});
