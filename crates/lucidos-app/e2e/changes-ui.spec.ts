import { test, expect } from './fixtures';
import {
  navigateToApp, uniqueMessage, assertHealthy, gotoWithRetry, clickChangeAction,
} from './helpers';
import {
  createCCThreadWithChange, cleanupCCThread, cleanupFileFromMain, psql, WORKSPACE,
} from './db-helpers';

test.describe('Claude Code changes - apply and discard via UI', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('pending change shows in changes API', async ({ page }) => {
    const suffix = uniqueMessage('ui-api').replace(/[^a-z0-9-]/g, '');
    const { threadId, changeId, branch, file } = createCCThreadWithChange('E2E Change Test', suffix);

    try {
      const resp = await page.request.get('/api/v1/changes');
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
      await page.addInitScript((tid: string) => {
        localStorage.setItem('lucidos-focused-thread', tid);
      }, threadId);
      await navigateToApp(page);

      await clickChangeAction(page, 'Apply');

      await page.waitForFunction(async (cid) => {
        const resp = await fetch('/api/v1/changes');
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

      await expect(page.locator('.thread-action-buttons:visible button:has-text("Apply")').first())
        .toBeVisible({ timeout: 15_000 });
      await clickChangeAction(page, 'Discard');

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

test.describe('Changes panel infinite scroll', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('scrolling the applied list loads older pages beyond the first', async ({ page }) => {
    const marker = uniqueMessage('scroll-more').replace(/[^a-z0-9-]/g, '');
    const TOTAL = 30;
    psql(
      `DELETE FROM changes WHERE status IN ('applied', 'reverted');\n` +
        `INSERT INTO changes ` +
        `(id, request_id, branch_name, repo_root, description, file_count, files, requires_restart, hardened, status, created_at, resolved_at) ` +
        `SELECT gen_random_uuid(), gen_random_uuid(), 'e2e-scroll-' || g || '-${marker}', '${WORKSPACE}', ` +
        `'Applied change ${marker} #' || g, 1, ARRAY['e2e-${marker}-' || g || '.txt'], false, true, 'applied', ` +
        `NOW() - (g || ' seconds')::interval, NOW() - (g || ' seconds')::interval ` +
        `FROM generate_series(1, ${TOTAL}) AS g`,
    );

    try {
      await page.addInitScript(() => {
        localStorage.setItem('lucidos-active-menu-item', 'changes');
        localStorage.setItem('lucidos-mobile-view', 'content');
      });
      await gotoWithRetry(page, '/');

      const visibleCount = () =>
        page.evaluate((m) => {
          const els = document.querySelectorAll('.change-row');
          return Array.from(els).filter((el) => {
            const r = el.getBoundingClientRect();
            return r.width > 0 && r.height > 0 && (el.textContent || '').includes(m);
          }).length;
        }, marker);

      await expect.poll(visibleCount, { timeout: 15_000 }).toBe(15);

      await expect
        .poll(
          async () => {
            await page.evaluate(() => {
              const els = document.querySelectorAll('.content-pane-body');
              for (const el of els) {
                const r = el.getBoundingClientRect();
                if (r.width > 0 && r.height > 0) {
                  el.scrollTop = el.scrollHeight;
                  return;
                }
              }
            });
            return visibleCount();
          },
          { timeout: 15_000, intervals: [200, 300, 500, 1000] },
        )
        .toBe(TOTAL);
    } finally {
      psql(`DELETE FROM changes WHERE description LIKE '%${marker}%'`);
    }
  });
});
