import { test, expect } from '@playwright/test';
import {
  navigateToApp, uniqueMessage, assertHealthy, gotoWithRetry,
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
    // Regression: the "Scroll for more" affordance never loaded the next page
    // of Recently Applied changes. The load-more trigger was an `onScroll`
    // handler bound to the inner `.panel-content`, which has no overflow and
    // never scrolls — the real scroll container is the ancestor
    // `.content-pane-body`, and scroll events don't bubble. So the list stayed
    // stuck at the first 15. Mirrors the notifications infinite-scroll fix
    // (IntersectionObserver rooted at `.content-pane-body`).
    const marker = uniqueMessage('scroll-more').replace(/[^a-z0-9-]/g, '');
    const TOTAL = 30;
    // Clear pre-existing applied/reverted rows so the seeded set is the only
    // thing in the Recently Applied list — first page is then exactly 15 and
    // the full list exactly TOTAL, with no ties at the cursor boundary
    // (strictly-decreasing resolved_at per row).
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
      // Land directly on the Changes panel (content pane on mobile).
      await page.addInitScript(() => {
        localStorage.setItem('lucidos-active-menu-item', 'changes');
        sessionStorage.setItem('lucidos-mobile-view', 'content');
      });
      await gotoWithRetry(page, '/');

      // Count only physically-visible seeded rows (filter by marker so other
      // suites' rows and any leftover pending rows don't skew the total).
      const visibleCount = () =>
        page.evaluate((m) => {
          const els = document.querySelectorAll('.change-row');
          return Array.from(els).filter((el) => {
            const r = el.getBoundingClientRect();
            return r.width > 0 && r.height > 0 && (el.textContent || '').includes(m);
          }).length;
        }, marker);

      // First page renders exactly one page (15) — proves the list paginates
      // instead of dumping all 30 at once.
      await expect.poll(visibleCount, { timeout: 15_000 }).toBe(15);

      // Scroll the REAL scroll container (`.content-pane-body`) to the bottom
      // inside the poll so multi-page lists keep advancing each iteration. The
      // bug left this stuck at 15 forever.
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
