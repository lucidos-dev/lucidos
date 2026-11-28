import { test, expect } from './fixtures';
import {
  navigateToApp,
  newThread,
  sendMessage,
  waitForResponse,
  assertHealthy,
  revealSteps,
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

    await revealSteps(page);

    const counter = page
      .locator('[data-role="inline-step"]:visible [data-role="step-context"]')
      .first();
    await expect(counter).toBeVisible({ timeout: 30_000 });
    await counter.click();

    const modal = page.locator('[data-role="context-captured-modal"]:visible');
    await expect(modal).toBeVisible();
    await expect(modal.locator('[data-role="budget-bar"]')).toBeVisible();

    await expect(modal.locator('[data-role="section-row"]').first()).toBeVisible({ timeout: 10_000 });
    const sectionRows = modal.locator('[data-role="section-row"]');
    expect(await sectionRows.count()).toBeGreaterThan(1);

    const usageRow = modal.locator('[data-role="usage-row"]');
    if ((await usageRow.count()) > 0) {
      await expect(usageRow).toContainText(/input/i);
    }

    await modal.locator('[data-role="surface-close"]').click();
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

  test('ContextCaptured sections lazy-fetch on modal open after reload', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);

    await sendMessage(page, 'Say "hello world" and nothing else.');
    await waitForResponse(page);

    await page.reload();
    await assertHealthy(page);

    let lazyFetchCount = 0;
    page.on('request', req => {
      const u = req.url();
      if (/\/api\/v1\/events\/[^/]+\/context$/.test(u)) {
        lazyFetchCount += 1;
      }
    });

    await revealSteps(page);

    const counter = page
      .locator('[data-role="inline-step"]:visible [data-role="step-context"]')
      .first();
    await expect(counter).toBeVisible({ timeout: 30_000 });
    await counter.click();

    const modal = page.locator('[data-role="context-captured-modal"]:visible');
    await expect(modal).toBeVisible();
    await expect(modal.locator('[data-role="budget-bar"]')).toBeVisible();

    await expect(modal.locator('[data-role="section-row"]').first()).toBeVisible({ timeout: 10_000 });
    expect(await modal.locator('[data-role="section-row"]').count()).toBeGreaterThan(1);
    await expect(modal.locator('.loading-fade-skeleton:not(.loading-fade-out)')).toHaveCount(0);
    await expect(modal.locator('[data-role="context-sections-error"]')).toHaveCount(0);
    expect(lazyFetchCount).toBeGreaterThan(0);
  });
});
