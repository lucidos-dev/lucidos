import { test, expect } from './fixtures';
import { navigateToApp, uniqueMessage, assertHealthy } from './helpers';
import { createCCThreadWithChange, cleanupCCThread } from './db-helpers';

/**
 * Mobile-scoped (`-mobile` suffix: mobile + mobile-webkit, not chromium). The
 * action slot merges the close set (Apply + Discard) into one control. On a
 * phone it is the round button, and every tap opens its menu, Apply included,
 * so no tap applies at once (ADR 0573). Diff stays outside it, a standalone
 * icon. The desktop pill is covered by change-actions-split-desktop.
 */
test.describe('Change actions: the phone round button', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('a tap opens the menu with Apply and Discard; Diff is standalone; Discard works', async ({ page }) => {
    const suffix = uniqueMessage('split').replace(/[^a-z0-9-]/g, '');
    const { threadId, changeId, branch, file } = createCCThreadWithChange(
      'E2E Split Button', suffix, { requiresRestart: true },
    );

    try {
      await page.addInitScript((tid: string) => {
        localStorage.setItem('lucidos-focused-thread', tid);
      }, threadId);
      await navigateToApp(page);

      // One round button, named for its face (the asterisk marks the
      // restart-requiring change). No caret: the whole button opens the menu.
      const face = page.locator('.thread-action-buttons:visible .split-button-whole');
      await expect(face).toBeVisible({ timeout: 15_000 });
      await expect(face).toHaveAttribute('aria-label', /^Apply\*\./);
      await expect(page.locator('.thread-action-buttons:visible .split-button-caret')).toHaveCount(0);
      const box = (await face.boundingBox())!;
      expect(Math.abs(box.width - box.height)).toBeLessThan(1);

      // Diff is a standalone top-level icon, never folded into the menu.
      await expect(page.locator('.thread-action-buttons:visible button[data-role="thread-diff"]')).toBeVisible();

      // A tap opens the menu and applies nothing.
      await face.click();
      const menu = page.locator('.split-button-menu:visible');
      await expect(menu).toBeVisible();
      await expect(menu.locator('button[role="menuitem"]')).toHaveText([/^Discard$/, /^Set aside$/, /^Apply\*$/]);
      await expect(menu.locator('button[data-role="thread-diff"]')).toHaveCount(0);
      const pending = await page.evaluate(async (cid) => {
        const body = await (await fetch('/api/v1/changes')).json();
        return (body.pending as Array<{ id: string }>).some(c => c.id === cid);
      }, changeId);
      expect(pending).toBe(true);

      // Discarding via the menu drops the change from the pending list.
      await menu.locator('button:has-text("Discard")').first().click();
      const confirmBtn = page.locator('[data-role="confirm-ok"]:visible').first();
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
