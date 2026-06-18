import { test, expect } from './fixtures';
import { assertHealthy, navigateToApp, ensureOnThreadPane, uniqueMessage } from './helpers';
import { createCCThreadWithChange, cleanupCCThread } from './db-helpers';

// Verifies the measure-driven lift in PromptInput. With a CC banner showing
// [Save][Diff][Discard][Apply] alongside the prompt's left icons, the natural
// single-row layout overflows below ~330px of available width. useFitsInOneRow
// detects this via getBoundingClientRect on every [data-row-item] and the JSX
// switches .prompt-actions-right into a stacked column with two
// .prompt-actions-subrow children — only Diff lifts, Save stays anchored to
// the bottom row alongside Discard and Apply. On a wider viewport the row
// fits and prompt-actions-right collapses back to a single inline row.
//
// The measurement is real (no viewport-width heuristics), so resizing the
// viewport flips the layout in both directions. We pick 320 (below the
// fits-in-row threshold) and 1280 (well above) to get a deterministic flip on
// the default font size; users with Display Zoom or larger Dynamic Type land
// on the same code path at wider viewports too.

test.describe('Prompt actions row — measure-driven lift', () => {

  // Mobile Playwright projects (`mobile`, `mobile-webkit`) hard-pin their
  // viewport via device emulation, so setViewportSize behaves inconsistently.
  // The `-desktop.spec.ts` filename is matched by `testIgnore` on those
  // projects so they never enter this file — only chromium runs it.
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('Diff lifts to a sub-row above the primary actions when the row would overflow, drops back inline when it fits', async ({ page }) => {
    const suffix = uniqueMessage('lift').replace(/[^a-z0-9-]/g, '');
    const { threadId, changeId, branch, file } = createCCThreadWithChange('Lift test', suffix);

    try {
      await page.setViewportSize({ width: 320, height: 800 });
      await page.addInitScript((tid: string) => {
        localStorage.setItem('lucidos-focused-thread', tid);
      }, threadId);
      await navigateToApp(page);
      await ensureOnThreadPane(page);

      // Wait for the banner to render so the prompt-actions-row has its full
      // item set before we measure layout.
      await expect(page.locator('.thread-action-buttons:visible')).toBeVisible({ timeout: 15_000 });
      await expect(page.locator('.thread-action-buttons:visible button:has-text("Apply")')).toBeVisible();

      // Narrow viewport: prompt-actions-right enters its stacked variant with
      // two sub-rows; the top one carries Diff alone. Use :visible to scope to
      // the active layout — SplitLayout + MobileSwipeContainer both render
      // PromptInput; the hidden one's children have 0x0 rects.
      const stackedRight = page.locator('.prompt-actions-right.is-stacked:visible');
      await expect(stackedRight).toBeVisible({ timeout: 5_000 });
      const liftSubrow = stackedRight.locator('.prompt-actions-subrow').first();
      await expect(liftSubrow.locator('button:has-text("Diff")')).toBeVisible();
      // Save must NOT ride above with Diff — there's room for it on the bottom.
      await expect(liftSubrow.locator('button:has-text("Save")')).toHaveCount(0);

      // Bottom sub-row carries Save + Discard + Apply (in that order), and
      // Diff is NOT duplicated there.
      const primarySubrow = stackedRight.locator('.prompt-actions-subrow').nth(1);
      await expect(primarySubrow.locator('button:has-text("Save")')).toBeVisible();
      await expect(primarySubrow.locator('button:has-text("Discard")')).toBeVisible();
      await expect(primarySubrow.locator('button:has-text("Apply")')).toBeVisible();
      await expect(primarySubrow.locator('button:has-text("Diff")')).toHaveCount(0);

      // Grow to a desktop width — the row now fits, the column collapses back
      // to a flat inline row.
      await page.setViewportSize({ width: 1280, height: 800 });
      await expect(page.locator('.prompt-actions-right.is-stacked:visible')).toHaveCount(0, { timeout: 5_000 });
      await expect(page.locator('.prompt-actions-right:visible button:has-text("Diff")')).toBeVisible();

      // Shrink back — the stack returns. Confirms the resize-driven loop holds
      // in both directions.
      await page.setViewportSize({ width: 320, height: 800 });
      await expect(page.locator('.prompt-actions-right.is-stacked:visible .prompt-actions-subrow').first().locator('button:has-text("Diff")')).toBeVisible({ timeout: 5_000 });
    } finally {
      cleanupCCThread(threadId, changeId, branch, file);
    }
  });

  // Apply & Restart pushes the bottom sub-row past a phone-width container,
  // so Save lifts alongside Diff and the bottom keeps [Discard][Apply & Restart]
  // — the row that still fits.
  test('Save lifts alongside Diff when Apply gains the "& Restart" suffix', async ({ page }) => {
    const suffix = uniqueMessage('lift-restart').replace(/[^a-z0-9-]/g, '');
    const { threadId, changeId, branch, file } = createCCThreadWithChange('Lift restart test', suffix, { requiresRestart: true });

    try {
      await page.setViewportSize({ width: 320, height: 800 });
      await page.addInitScript((tid: string) => {
        localStorage.setItem('lucidos-focused-thread', tid);
      }, threadId);
      await navigateToApp(page);
      await ensureOnThreadPane(page);

      await expect(page.locator('.thread-action-buttons:visible')).toBeVisible({ timeout: 15_000 });
      await expect(page.locator('.thread-action-buttons:visible button:has-text("Apply & Restart")')).toBeVisible();

      const stackedRight = page.locator('.prompt-actions-right.is-stacked:visible');
      await expect(stackedRight).toBeVisible({ timeout: 5_000 });

      // Top sub-row carries Save + Diff (Save lifted to keep the long Apply
      // label on a row that fits).
      const liftSubrow = stackedRight.locator('.prompt-actions-subrow').first();
      await expect(liftSubrow.locator('button:has-text("Save")')).toBeVisible();
      await expect(liftSubrow.locator('button:has-text("Diff")')).toBeVisible();

      // Bottom sub-row carries Discard + Apply & Restart only — Save is NOT
      // duplicated there.
      const primarySubrow = stackedRight.locator('.prompt-actions-subrow').nth(1);
      await expect(primarySubrow.locator('button:has-text("Discard")')).toBeVisible();
      await expect(primarySubrow.locator('button:has-text("Apply & Restart")')).toBeVisible();
      await expect(primarySubrow.locator('button:has-text("Save")')).toHaveCount(0);
    } finally {
      cleanupCCThread(threadId, changeId, branch, file);
    }
  });
});
