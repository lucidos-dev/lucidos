import { test, expect } from '@playwright/test';
import {
  navigateToApp, uniqueMessage, assertHealthy,
} from './helpers';
import {
  createCCThreadWithChange, cleanupCCThread, cleanupFileFromMain,
} from './db-helpers';

test.describe('Claude Code changes - apply and discard via UI', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('pending change shows in changes API', async ({ page }) => {
    const suffix = uniqueMessage('ui-api').replace(/[^a-z0-9-]/g, '');
    const { threadId, changeId, branch, file } = createCCThreadWithChange('E2E Change Test', suffix);

    try {
      const resp = await page.request.get('/api/changes');
      const body = await resp.json();
      const pending = body.pending as Array<{ id: string; description: string }>;
      const found = pending.find(c => c.id === changeId);
      expect(found).toBeDefined();
      expect(found!.description).toContain(suffix);
    } finally {
      cleanupCCThread(threadId, changeId, branch, file);
    }
  });

  test('apply a pending change via Apply button in action panel', async ({ page }) => {
    const suffix = uniqueMessage('ui-apply').replace(/[^a-z0-9-]/g, '');
    const { threadId, changeId, branch, file } = createCCThreadWithChange('E2E Change Test', suffix);

    try {
      // Focus the thread via localStorage before navigating
      await page.addInitScript((tid: string) => {
        localStorage.setItem('lucidos-focused-thread', tid);
      }, threadId);
      await navigateToApp(page);

      // The action panel with Apply/Discard should appear for this CC thread
      const applyBtn = page.locator('.thread-action-buttons:visible button.action-btn-confirm:has-text("Apply")').first();
      await expect(applyBtn).toBeVisible({ timeout: 15_000 });
      await applyBtn.click();

      // Wait for the change to leave the pending list
      await page.waitForFunction(async (cid) => {
        const resp = await fetch('/api/changes');
        const body = await resp.json();
        return !(body.pending as Array<{ id: string }>).find(c => c.id === cid);
      }, changeId, { timeout: 15_000 });

      cleanupFileFromMain(file, suffix);
    } finally {
      cleanupCCThread(threadId, changeId, branch, file);
    }
  });

  test('discard a pending change via Discard button in action panel', async ({ page }) => {
    const suffix = uniqueMessage('ui-discard').replace(/[^a-z0-9-]/g, '');
    const { threadId, changeId, branch, file } = createCCThreadWithChange('E2E Change Test', suffix);

    try {
      await page.addInitScript((tid: string) => {
        localStorage.setItem('lucidos-focused-thread', tid);
      }, threadId);
      await navigateToApp(page);

      const discardBtn = page.locator('.thread-action-buttons:visible button.action-btn-danger:has-text("Discard")').first();
      await expect(discardBtn).toBeVisible({ timeout: 15_000 });
      await discardBtn.click();

      // Handle confirmation dialog if present
      const confirmBtn = page.locator('.confirm-btn-ok:visible, .confirm-btn-ok-default:visible').first();
      if (await confirmBtn.isVisible({ timeout: 3_000 }).catch(() => false)) {
        await confirmBtn.click();
      }

      // Wait for the change to leave the pending list
      await page.waitForFunction(async (cid) => {
        const resp = await fetch('/api/changes');
        const body = await resp.json();
        return !(body.pending as Array<{ id: string }>).find(c => c.id === cid);
      }, changeId, { timeout: 15_000 });
    } finally {
      cleanupCCThread(threadId, changeId, branch, file);
    }
  });
});
