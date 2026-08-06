import { test, expect } from './fixtures';
import {
  navigateToApp,
  newThread,
  sendMessage,
  waitForResponse,
  assertHealthy,
} from './helpers';

test.describe('ContextCaptured modal', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('unified ContextCaptured panel renders after a chat turn', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);

    await sendMessage(page, 'Say "hello world" and nothing else.');
    await waitForResponse(page);

    // Inline steps are hidden by default (`stepsExpanded` in localStorage).
    const showStepsBtn = page
      .locator('button.details-toggle:visible', { hasText: 'Show steps' })
      .first();
    await expect(showStepsBtn).toBeVisible({ timeout: 30_000 });
    await showStepsBtn.click();

    // The context counter on the step row is the viewer's only door: the rest
    // of the row opens the step detail instead (asserted at the end).
    const counter = page
      .locator('[data-role="inline-step"]:visible [data-role="step-context"]')
      .first();
    await expect(counter).toBeVisible({ timeout: 30_000 });
    await counter.click();

    const modal = page.locator('[data-role="context-captured-modal"]:visible');
    await expect(modal).toBeVisible();
    await expect(modal.locator('[data-role="budget-bar"]')).toBeVisible();

    // Live SSE delivers full sections; reload-after-fetch covers the lazy path.
    // First section appears as soon as either the in-memory snap or the
    // /api/v1/events/:event_id/context fetch settles.
    await expect(modal.locator('[data-role="section-row"]').first()).toBeVisible({ timeout: 10_000 });
    const sectionRows = modal.locator('[data-role="section-row"]');
    expect(await sectionRows.count()).toBeGreaterThan(1);

    // Mock LLM (LUCIDOS_MODEL=mock, see lib/e2e.sh) returns None for
    // usage, so the row may legitimately be absent.
    const usageRow = modal.locator('[data-role="usage-row"]');
    if ((await usageRow.count()) > 0) {
      await expect(usageRow).toContainText(/input/i);
    }

    // The other half of the split: the rest of the row opens what the step DID,
    // and that view must NOT carry a second copy of the context. A duplicate
    // there is what would make the counter a pointless door.
    await modal.locator('.step-detail-close').click();
    await expect(modal).toHaveCount(0);
    await page
      .locator('[data-role="inline-step"]:visible [data-role="step-main"]')
      .first()
      .click();
    const stepDetail = page.locator('[data-role="step-detail-modal"]:visible');
    await expect(stepDetail).toBeVisible();
    await expect(stepDetail.locator('[data-role="budget-bar"]')).toHaveCount(0);
    await expect(stepDetail.locator('[data-role="section-row"]')).toHaveCount(0);
  });

  // The snapshot endpoint strips ContextCaptured.sections + tools (api/threads.rs
  // :: strip_context_capture_sections) so the events list stays small even when
  // the thread has hundreds of captures. After a reload the chip still renders
  // (lightweight fields preserved), and opening the step modal lazy-fetches
  // sections via GET /api/v1/events/:event_id/context.
  test('ContextCaptured sections lazy-fetch on modal open after reload', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);

    await sendMessage(page, 'Say "hello world" and nothing else.');
    await waitForResponse(page);

    // Reload forces the snapshot path; live-SSE captures are gone from memory.
    await page.reload();
    await assertHealthy(page);

    // Count the lazy-fetch call so we can prove it actually fired — a
    // regression that bypasses the strip would render sections inline and
    // this counter would stay 0, failing the test. We use `page.on('request')`
    // rather than `page.route()` because the service worker (sw.js) intercepts
    // GET /api/v1/* fetches and re-issues them itself; per Playwright docs
    // `page.route()` does NOT intercept service-worker requests, so the route
    // handler would never fire and the assertion would silently fail.
    let lazyFetchCount = 0;
    page.on('request', req => {
      const u = req.url();
      if (/\/api\/v1\/events\/[^/]+\/context$/.test(u)) {
        lazyFetchCount += 1;
      }
    });

    const showStepsBtn = page
      .locator('button.details-toggle:visible', { hasText: 'Show steps' })
      .first();
    await expect(showStepsBtn).toBeVisible({ timeout: 30_000 });
    await showStepsBtn.click();

    const counter = page
      .locator('[data-role="inline-step"]:visible [data-role="step-context"]')
      .first();
    await expect(counter).toBeVisible({ timeout: 30_000 });
    await counter.click();

    const modal = page.locator('[data-role="context-captured-modal"]:visible');
    await expect(modal).toBeVisible();
    // Budget bar renders immediately from the lightweight snap fields.
    await expect(modal.locator('[data-role="budget-bar"]')).toBeVisible();

    // Sections appear once the lazy-fetch resolves. The "Loading sections…"
    // placeholder may flash; the section rows are the lazy-load done signal.
    await expect(modal.locator('[data-role="section-row"]').first()).toBeVisible({ timeout: 10_000 });
    expect(await modal.locator('[data-role="section-row"]').count()).toBeGreaterThan(1);
    // Loading + error indicators must clear after the fetch resolves.
    await expect(modal.locator('[data-role="context-sections-loading"]')).toHaveCount(0);
    await expect(modal.locator('[data-role="context-sections-error"]')).toHaveCount(0);
    // The strip + lazy-fetch contract was exercised end-to-end — at least
    // one GET /api/v1/events/:eid/context must have fired. A regression that
    // disables stripping would render sections inline and this would stay 0.
    expect(lazyFetchCount).toBeGreaterThan(0);
  });
});
