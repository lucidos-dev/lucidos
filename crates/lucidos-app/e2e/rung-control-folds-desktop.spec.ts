import { test, expect, type Page } from './fixtures';
import {
  assertHealthy, navigateToApp, ensureOnThreadPane, uniqueMessage, newThread, waitForVisibleInput,
  COMPOSER_FOLD_TRIGGER, hideDashboardRail,
} from './helpers';
import { createCCThreadWithChange, cleanupCCThread } from './db-helpers';

// The row folds left to right, and the rung pill stands right of every icon.
// So the Claude menu leaves before it. A row too narrow for the pill puts the
// pill in the ⋯ too, and its menu row still opens the rung list. A narrow
// compose row has no change actions, so it folds every other member and keeps
// the pill. The pill's compact face never shortens.
// Desktop-only (`-desktop.spec.ts`): see change-actions-split-desktop.spec.ts.

/** Cap the composer row at `content` px of content, its own padding added. */
async function capRow(page: Page, content: number): Promise<void> {
  const padding = await page.locator('.prompt-actions-row:visible').first().evaluate((el) => {
    const cs = getComputedStyle(el);
    return parseFloat(cs.paddingLeft) + parseFloat(cs.paddingRight);
  });
  await page.addStyleTag({ content: `.prompt-actions-row { max-width: ${Math.ceil(content + padding)}px; }` });
}

/** A visible box's width in the composer row. */
async function widthIn(page: Page, selector: string): Promise<number> {
  return (await page.locator('.prompt-actions-row:visible').locator(selector).first().boundingBox())!.width;
}

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
      // The phone face, the badge alone, fits a 320px row. Cap the row below the
      // badge and the action button together, but above the ⋯ and the button.
      await expect(page.locator('.thread-action-buttons:visible')).toBeVisible({ timeout: 15_000 });
      await capRow(page, await widthIn(page, '.prompt-actions-right > :last-child') + 1.5 * await widthIn(page, '.icon-btn'));

      const row = page.locator('.prompt-actions-row:visible');
      await expect(row.locator('.rung-pill')).toHaveCount(0);
      // The Claude menu stands left of the pill, so it folded first.
      await expect(row.locator('.commands-btn')).toHaveCount(0);
      await row.locator('[data-fold-key=""]').click();
      await page.locator('.thread-overflow-item[data-role="rung-control"]:visible').click();
      await expect(page.locator('.control-dropdown:visible .control-item').first()).toBeVisible();
    } finally {
      cleanupCCThread(threadId, changeId, branch, file);
    }
  });

  test('a narrow compose row folds every other member and keeps the pill', async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 800 });
    // The rail would squeeze the thread pane until the pill folds by itself.
    await hideDashboardRail(page);
    await navigateToApp(page);
    await newThread(page);
    await ensureOnThreadPane(page);
    const input = await waitForVisibleInput(page);
    await input.fill(uniqueMessage('rung-compose'));
    // A row that narrow draws the compact faces (ADR 0576), so measure those.
    // A 30rem row is compact; the second cap's later style tag wins.
    await expect(page.locator('.prompt-actions-row:visible .rung-pill:not([data-pending])')).toBeVisible();
    await capRow(page, 30 * await page.evaluate(() => parseFloat(getComputedStyle(document.documentElement).fontSize)));
    await expect(page.locator('.prompt-actions-row:visible')).toHaveAttribute('data-composer-compact', 'true');
    // Cap the row where the ⋯, the pill and Send fit, with the cluster's gap,
    // but one more icon does not. Measured, so the fold never depends on fonts.
    const icon = await widthIn(page, '.icon-btn');
    await capRow(page, await widthIn(page, '.rung-pill') + await widthIn(page, '.send-cancel-morph') + 8 + 1.5 * icon);

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
