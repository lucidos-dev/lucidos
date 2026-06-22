import { test, expect } from './fixtures';
import { navigateToApp, uniqueMessage, assertHealthy } from './helpers';
import { createCCThreadWithChange, cleanupCCThread } from './db-helpers';

/**
 * Mobile-scoped (`-mobile` suffix → mobile + mobile-webkit, not chromium): the
 * WaitingBanner collapses the change-action cluster into a split button on every
 * viewport — a one-tap "Apply & Restart" primary face plus a caret that opens an
 * upward menu holding Diff and Discard. This verifies the split renders + works
 * at a real mobile viewport; the desktop rendering of the same control is
 * covered by change-actions-split-desktop / diff-button-branch-has-diff-desktop.
 */
test.describe('Change actions — mobile split button', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('Apply is the face; Diff + Discard live in the caret menu; Discard works', async ({ page }) => {
    const suffix = uniqueMessage('split').replace(/[^a-z0-9-]/g, '');
    const { threadId, changeId, branch, file } = createCCThreadWithChange(
      'E2E Split Button', suffix, { requiresRestart: true },
    );

    try {
      await page.addInitScript((tid: string) => {
        localStorage.setItem('lucidos-focused-thread', tid);
      }, threadId);
      await navigateToApp(page);

      // The primary face is the one-tap Apply (& Restart) — a real button, not
      // hidden behind the menu.
      const face = page.locator('.thread-action-buttons:visible .split-button-primary');
      await expect(face).toBeVisible({ timeout: 15_000 });
      await expect(face).toHaveText(/Apply & Restart/);

      // Discard and Diff are NOT top-level buttons on mobile — they fold into
      // the (closed) caret menu, so they aren't rendered yet.
      await expect(page.locator('.thread-action-buttons:visible button.action-btn-danger')).toHaveCount(0);
      await expect(page.locator('.thread-action-buttons:visible button:has-text("Diff")')).toHaveCount(0);

      // Open the caret menu and assert it lists Diff + Discard.
      await page.locator('.thread-action-buttons:visible .split-button-caret').first().click();
      const menu = page.locator('.split-button-menu:visible');
      await expect(menu).toBeVisible();
      await expect(menu.locator('button:has-text("Diff")')).toBeVisible();
      await expect(menu.locator('button:has-text("Discard")')).toBeVisible();

      // Discarding via the menu drops the change from the pending list.
      await menu.locator('button:has-text("Discard")').first().click();
      const confirmBtn = page.locator('.confirm-btn-ok:visible, .confirm-btn-ok-default:visible').first();
      if (await confirmBtn.isVisible({ timeout: 3_000 }).catch(() => false)) {
        await confirmBtn.click();
      }
      await page.waitForFunction(async (cid) => {
        const resp = await fetch('/api/v1/changes');
        const body = await resp.json();
        return !(body.pending as Array<{ id: string }>).find(c => c.id === cid);
      }, changeId, { timeout: 15_000 });
    } finally {
      cleanupCCThread(threadId, changeId, branch, file);
    }
  });
});
