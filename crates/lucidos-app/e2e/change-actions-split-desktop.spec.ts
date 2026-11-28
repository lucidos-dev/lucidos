import { test, expect } from './fixtures';
import type { Page } from './fixtures';
import { assertHealthy, hideDashboardRail, navigateToApp, ensureOnThreadPane, uniqueMessage } from './helpers';
import { createCCThreadWithChange, cleanupCCThread } from './db-helpers';


test.describe('Prompt actions row — Apply/Discard split button, standalone Diff', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  async function openSplitMenu(page: Page) {
    const banner = page.locator('.thread-action-buttons:visible');
    await expect(banner).toBeVisible({ timeout: 15_000 });
    await expect(banner.locator('.split-button-primary')).toBeVisible();
    await expect(banner.locator('button[data-role="thread-diff"]')).toBeVisible();
    await expect(banner.locator('button.action-btn-danger')).toHaveCount(0);

    await banner.locator('.split-button-caret').first().click();
    const menu = page.locator('.split-button-menu:visible');
    await expect(menu).toBeVisible();
    return menu;
  }

  test('at 320px the round button opens Apply and Discard; Diff stays a standalone button', async ({ page }) => {
    const suffix = uniqueMessage('split').replace(/[^a-z0-9-]/g, '');
    const { threadId, changeId, branch, file } = createCCThreadWithChange('Split test', suffix);

    try {
      await page.setViewportSize({ width: 320, height: 800 });
      await page.addInitScript((tid: string) => {
        localStorage.setItem('lucidos-focused-thread', tid);
      }, threadId);
      await navigateToApp(page);
      await ensureOnThreadPane(page);

      const banner = page.locator('.thread-action-buttons:visible');
      await expect(banner.locator('button[data-role="thread-diff"]')).toBeVisible({ timeout: 15_000 });
      await banner.locator('.split-button-whole').click();
      const menu = page.locator('.split-button-menu:visible');
      await expect(menu.locator('button[role="menuitem"]')).toHaveText([/^Discard$/, /^Hold…$/, /^Apply$/]);
      await expect(menu.locator('button[data-role="thread-diff"]')).toHaveCount(0);
    } finally {
      cleanupCCThread(threadId, changeId, branch, file);
    }
  });

  test('the Apply face carries an asterisk (Apply*) when a restart is required', async ({ page }) => {
    const suffix = uniqueMessage('split-restart').replace(/[^a-z0-9-]/g, '');
    const { threadId, changeId, branch, file } = createCCThreadWithChange('Split restart test', suffix, { requiresRestart: true });

    try {
      await page.setViewportSize({ width: 2200, height: 1000 });
      await hideDashboardRail(page);
      await page.addInitScript((tid: string) => {
        localStorage.setItem('lucidos-focused-thread', tid);
      }, threadId);
      await navigateToApp(page);
      await ensureOnThreadPane(page);

      await expect(page.locator('.thread-action-buttons:visible .split-button-primary .action-slot-label')).toHaveText(/^Apply\*$/);
      const menu = await openSplitMenu(page);
      await expect(menu.locator('button[data-role="thread-diff"]')).toHaveCount(0);
      await expect(menu.locator('button:has-text("Discard")')).toBeVisible();
    } finally {
      cleanupCCThread(threadId, changeId, branch, file);
    }
  });
});
