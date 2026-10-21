import { test, expect } from './fixtures';
import {
  assertHealthy, navigateToApp, ensureOnThreadPane, uniqueMessage, newThread, waitForVisibleInput,
  COMPOSER_FOLD_TRIGGER,
} from './helpers';
import { createCCThreadWithChange, cleanupCCThread } from './db-helpers';

// A started thread's rung pill is the FIRST member the row folds into the ⋯
// menu. On a 320px row it goes there beside the change actions, and its menu
// row still opens the rung list. Before the first send it is the LAST, so a
// narrow compose row folds everything else and keeps the pill.
// Desktop-only (`-desktop.spec.ts`): see change-actions-split-desktop.spec.ts.

test.describe('The rung pill folds into the ⋯ menu', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('a ⋯ menu row opens the rung list when the row is too narrow for the pill', async ({ page }) => {
    const suffix = uniqueMessage('rung-fold').replace(/[^a-z0-9-]/g, '');
    const { threadId, changeId, branch, file } = createCCThreadWithChange('Rung fold test', suffix);

    try {
      await page.setViewportSize({ width: 320, height: 800 });
      await page.addInitScript((tid: string) => {
        localStorage.setItem('lucidos-focused-thread', tid);
      }, threadId);
      await navigateToApp(page);
      await ensureOnThreadPane(page);

      const row = page.locator('.prompt-actions-row:visible');
      await expect(row.locator('.rung-pill')).toHaveCount(0);
      await row.locator('[data-fold-key=""]').click();
      await page.locator('.thread-overflow-item[data-role="rung-control"]:visible').click();
      await expect(page.locator('.control-dropdown:visible .control-item').first()).toBeVisible();
    } finally {
      cleanupCCThread(threadId, changeId, branch, file);
    }
  });

  test('a narrow compose row folds every other member and keeps the pill', async ({ page }) => {
    // The default split leaves the thread pane a narrow row, and a draft
    // brings the clear button, so something has to fold.
    await page.setViewportSize({ width: 1280, height: 800 });
    await navigateToApp(page);
    await newThread(page);
    await ensureOnThreadPane(page);
    const input = await waitForVisibleInput(page);
    await input.fill(uniqueMessage('rung-compose'));

    try {
      const row = page.locator('.prompt-actions-row:visible');
      const more = page.locator(COMPOSER_FOLD_TRIGGER).first();
      await expect(more).toBeVisible();
      await expect(row.locator('.rung-pill')).toBeVisible();
      await more.click();
      await expect(page.locator('.thread-overflow-item:visible').first()).toBeVisible();
      await expect(page.locator('.thread-overflow-item[data-role="rung-control"]:visible')).toHaveCount(0);
      await page.keyboard.press('Escape');
    } finally {
      await input.fill('');
    }
  });
});
