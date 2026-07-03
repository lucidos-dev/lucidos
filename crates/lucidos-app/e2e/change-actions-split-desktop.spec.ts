import { test, expect } from './fixtures';
import type { Page } from './fixtures';
import { assertHealthy, navigateToApp, ensureOnThreadPane, uniqueMessage } from './helpers';
import { createCCThreadWithChange, cleanupCCThread } from './db-helpers';

// The close-set actions (Apply + Discard) collapse into a split button — a
// one-tap Apply face + a caret menu holding Discard — on every viewport. The
// Diff button lives permanently OUTSIDE that cluster as its own standalone
// top-level button (it lifts to a row above when the prompt row is too narrow).
// This spec pins that shape: the split button carries Apply + Discard, Diff is a
// standalone button, and Diff is never folded into the caret menu.
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
    // Diff is a standalone top-level button — never folded into the caret menu.
    await expect(banner.locator('button:has-text("Diff")')).toBeVisible();
    // Discard is not a top-level button — it folds into the caret menu.
    await expect(banner.locator('button.action-btn-danger')).toHaveCount(0);

    await banner.locator('.split-button-caret').first().click();
    const menu = page.locator('.split-button-menu:visible');
    await expect(menu).toBeVisible();
    return menu;
  }

  test('at 320px the split button holds Apply/Discard; Diff stays a standalone button', async ({ page }) => {
    const suffix = uniqueMessage('split').replace(/[^a-z0-9-]/g, '');
    const { threadId, changeId, branch, file } = createCCThreadWithChange('Split test', suffix);

    try {
      await page.setViewportSize({ width: 320, height: 800 });
      await page.addInitScript((tid: string) => {
        localStorage.setItem('lucidos-focused-thread', tid);
      }, threadId);
      await navigateToApp(page);
      await ensureOnThreadPane(page);

      await expect(page.locator('.thread-action-buttons:visible .split-button-primary')).toHaveText(/^Apply$/);
      const menu = await openSplitMenu(page);
      // Diff never appears in the menu; Discard does.
      await expect(menu.locator('button:has-text("Diff")')).toHaveCount(0);
      await expect(menu.locator('button:has-text("Discard")')).toBeVisible();
    } finally {
      cleanupCCThread(threadId, changeId, branch, file);
    }
  });

  test('the Apply face carries an asterisk (Apply*) when a restart is required', async ({ page }) => {
    const suffix = uniqueMessage('split-restart').replace(/[^a-z0-9-]/g, '');
    const { threadId, changeId, branch, file } = createCCThreadWithChange('Split restart test', suffix, { requiresRestart: true });

    try {
      await page.setViewportSize({ width: 1280, height: 800 });
      await page.addInitScript((tid: string) => {
        localStorage.setItem('lucidos-focused-thread', tid);
      }, threadId);
      await navigateToApp(page);
      await ensureOnThreadPane(page);

      await expect(page.locator('.thread-action-buttons:visible .split-button-primary')).toHaveText(/^Apply\*$/);
      const menu = await openSplitMenu(page);
      await expect(menu.locator('button:has-text("Diff")')).toHaveCount(0);
      await expect(menu.locator('button:has-text("Discard")')).toBeVisible();
    } finally {
      cleanupCCThread(threadId, changeId, branch, file);
    }
  });
});
