import { test, expect } from '@playwright/test';
import { navigateToApp, assertHealthy, switchToClaudeMode } from './helpers';
import { createIframeAppFixture } from './db-helpers';

test.describe('App coding-agent thread — compose-view scope picker', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  // The scope picker (compose view, Claude mode) groups options into Lucidos
  // / external repos / installed apps. Once an app exists in the workspace,
  // the dropdown must surface it under the "Apps" header. End-to-end CC
  // spawn from the dropdown is exercised by app-coding-agent-wip-preview
  // (which uses a seeded thread to skip the LLM); this spec stays focused
  // on the picker contract because LLM-driven spawn from a chat message is
  // non-deterministic.
  test('scope picker exposes installed apps under an "Apps" group', async ({ page }) => {
    const appId = `e2e-scope-${Date.now()}`;
    const fixture = createIframeAppFixture(appId, {
      html: '<!doctype html><title>scope picker fixture</title>',
      js: '/* noop */',
      manifest: { id: appId, name: `${appId} fixture`, description: 'scope picker e2e' },
    });
    try {
      await navigateToApp(page);
      await switchToClaudeMode(page);

      // Open the cc-repo-selector dropdown — the picker is the only
      // .cc-repo-selector on the page.
      const trigger = page.locator('.cc-repo-selector .dropdown-trigger:visible').first();
      await expect(trigger).toBeVisible({ timeout: 10_000 });
      await trigger.click();

      // The "Apps" header is a disabled option; the app's name is a
      // selectable option below it. Asserting both proves the grouping.
      const menu = page.locator('.dropdown-menu:visible').first();
      await expect(menu).toBeVisible({ timeout: 5_000 });
      const appsHeader = menu.locator('.dropdown-option-header', { hasText: 'Apps' });
      await expect(appsHeader).toBeVisible();
      const appOption = menu.locator('.dropdown-option', { hasText: `${appId} fixture` });
      await expect(appOption).toBeVisible();
      // Lucidos is always present as the implicit default.
      await expect(menu.locator('.dropdown-option', { hasText: 'Lucidos' }).first()).toBeVisible();

      // Picking the app writes `{kind:'app',appId}` into localStorage under
      // the migrated key. Assert the write so a regression in the
      // option-value encoding (repo: vs app: prefix) doesn't silently send
      // the wrong scope on the next compose send.
      await appOption.click();
      const stored = await page.evaluate(
        () => localStorage.getItem('lucidos-cc-last-scope'),
      );
      expect(stored).toBeTruthy();
      const parsed = JSON.parse(stored!);
      expect(parsed.kind).toBe('app');
      expect(parsed.appId).toBe(appId);
    } finally {
      fixture.cleanup();
    }
  });
});
