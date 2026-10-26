import { test, expect } from './fixtures';
import type { Page } from './fixtures';
import { assertHealthy, hideDashboardRail, navigateToApp, ensureOnThreadPane, uniqueMessage } from './helpers';
import { createCCThreadWithChange, cleanupCCThread } from './db-helpers';

// The close-set actions (Apply + Discard) merge into the action slot's one
// control (ADR 0573). On the desktop layout it is a split pill: a one-tap Apply
// face and a caret menu holding Discard. On the phone layout it is the round
// button whose tap opens the menu. Diff lives OUTSIDE it as a standalone icon,
// and is never folded into the menu.
//
// Desktop-only (`-desktop.spec.ts`): the mobile Playwright projects hard-pin
// their viewport via device emulation, so setViewportSize behaves
// inconsistently there. `testIgnore` keeps them out of this file — only
// chromium runs it. The mobile rendering is covered by
// change-actions-split-mobile.spec.ts.

test.describe('Prompt actions row — Apply/Discard split button, standalone Diff', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  async function openSplitMenu(page: Page) {
    const banner = page.locator('.thread-action-buttons:visible');
    await expect(banner).toBeVisible({ timeout: 15_000 });
    // The primary face is a one-tap Apply, never behind the menu.
    await expect(banner.locator('.split-button-primary')).toBeVisible();
    // Diff is a standalone top-level icon, never folded into the caret menu.
    await expect(banner.locator('button[data-role="thread-diff"]')).toBeVisible();
    // Discard is not a top-level button — it folds into the caret menu.
    await expect(banner.locator('button.action-btn-danger')).toHaveCount(0);

    await banner.locator('.split-button-caret').first().click();
    const menu = page.locator('.split-button-menu:visible');
    await expect(menu).toBeVisible();
    return menu;
  }

  // A 320px window gets the phone layout: the round button, whose tap opens
  // the menu with Apply in it.
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
      await expect(menu.locator('button[role="menuitem"]')).toHaveText([/^Discard$/, /^Set aside$/, /^Apply$/]);
      await expect(menu.locator('button[data-role="thread-diff"]')).toHaveCount(0);
    } finally {
      cleanupCCThread(threadId, changeId, branch, file);
    }
  });

  test('the Apply face carries an asterisk (Apply*) when a restart is required', async ({ page }) => {
    const suffix = uniqueMessage('split-restart').replace(/[^a-z0-9-]/g, '');
    const { threadId, changeId, branch, file } = createCCThreadWithChange('Split restart test', suffix, { requiresRestart: true });

    try {
      // Wide enough that the thread pane keeps the desktop faces (ADR 0576).
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
