import { test, expect } from './fixtures';
import type { Page } from './fixtures';
import { assertHealthy, navigateToApp, ensureOnThreadPane, uniqueMessage } from './helpers';
import { createCCThreadWithChange, cleanupCCThread } from './db-helpers';

// The change-action cluster collapses into a single split button (a one-tap
// Apply face + a caret menu holding Diff and Discard) on every viewport. That
// single compact control replaces the old [Diff][Discard][Apply] row, so the
// measure-driven lift/stack the prompt row used to need for those three buttons
// no longer engages for a change thread — there is nothing to overflow. This
// spec pins that: even at a phone-narrow 320px desktop width the actions stay a
// single un-stacked split button, and the caret menu still holds Diff + Discard.
//
// Desktop-only (`-desktop.spec.ts`): the mobile Playwright projects hard-pin
// their viewport via device emulation, so setViewportSize behaves
// inconsistently there. `testIgnore` keeps them out of this file — only
// chromium runs it. The mobile rendering is covered by
// change-actions-split-mobile.spec.ts.

test.describe('Prompt actions row — change actions collapse into a split button', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  async function openSplitMenu(page: Page) {
    const banner = page.locator('.thread-action-buttons:visible');
    await expect(banner).toBeVisible({ timeout: 15_000 });
    // The primary face is a one-tap Apply, never behind the menu.
    await expect(banner.locator('.split-button-primary')).toBeVisible();
    // The row stays a single control — the old multi-button stack never engages.
    await expect(page.locator('.prompt-actions-right.is-stacked:visible')).toHaveCount(0);
    // Discard/Diff are not top-level buttons — they fold into the caret menu.
    await expect(banner.locator('button.action-btn-danger')).toHaveCount(0);
    await expect(banner.locator('button:has-text("Diff")')).toHaveCount(0);

    await banner.locator('.split-button-caret').first().click();
    const menu = page.locator('.split-button-menu:visible');
    await expect(menu).toBeVisible();
    return menu;
  }

  test('at 320px the actions are one split button; the caret menu holds Diff + Discard', async ({ page }) => {
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
      await expect(menu.locator('button:has-text("Diff")')).toBeVisible();
      await expect(menu.locator('button:has-text("Discard")')).toBeVisible();
    } finally {
      cleanupCCThread(threadId, changeId, branch, file);
    }
  });

  test('the Apply face carries the "& Restart" suffix when a restart is required', async ({ page }) => {
    const suffix = uniqueMessage('split-restart').replace(/[^a-z0-9-]/g, '');
    const { threadId, changeId, branch, file } = createCCThreadWithChange('Split restart test', suffix, { requiresRestart: true });

    try {
      await page.setViewportSize({ width: 1280, height: 800 });
      await page.addInitScript((tid: string) => {
        localStorage.setItem('lucidos-focused-thread', tid);
      }, threadId);
      await navigateToApp(page);
      await ensureOnThreadPane(page);

      await expect(page.locator('.thread-action-buttons:visible .split-button-primary')).toHaveText(/Apply & Restart/);
      const menu = await openSplitMenu(page);
      await expect(menu.locator('button:has-text("Diff")')).toBeVisible();
      await expect(menu.locator('button:has-text("Discard")')).toBeVisible();
    } finally {
      cleanupCCThread(threadId, changeId, branch, file);
    }
  });
});
