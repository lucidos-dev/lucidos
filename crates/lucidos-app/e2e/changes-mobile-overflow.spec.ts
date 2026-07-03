import { test, expect } from './fixtures';
import { assertHealthy, uniqueMessage, gotoWithRetry } from './helpers';
import { createCCThreadWithChange, cleanupCCThread } from './db-helpers';

test.describe('Changes panel — long thread title on mobile', () => {
  test.use({ viewport: { width: 375, height: 812 } });

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('long thread title does not push action buttons off-screen', async ({ page }) => {
    const suffix = uniqueMessage('long-title').replace(/[^a-z0-9-]/g, '');
    const longTitle = 'Misplaced section for thread: AI Memory and Context Redundancy';
    const { threadId, changeId, branch, file } = createCCThreadWithChange(longTitle, suffix);

    try {
      await page.addInitScript(() => {
        localStorage.setItem('lucidos-active-menu-item', 'changes');
        localStorage.setItem('lucidos-mobile-view', 'content');
      });
      await gotoWithRetry(page, '/');

      const row = page.locator(`.list-row:visible:has(.list-row-label:has-text("${longTitle}"))`).first();
      await expect(row).toBeVisible({ timeout: 15_000 });

      const apply = row.locator('button.action-btn-confirm:has-text("Apply")');
      await expect(apply).toBeVisible();

      const box = await apply.boundingBox();
      const viewport = page.viewportSize()!;
      expect(box).not.toBeNull();
      expect(box!.x).toBeGreaterThanOrEqual(0);
      expect(box!.x + box!.width).toBeLessThanOrEqual(viewport.width);
    } finally {
      cleanupCCThread(threadId, changeId, branch, file);
    }
  });
});
