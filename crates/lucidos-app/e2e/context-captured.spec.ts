import { test, expect } from '@playwright/test';
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

    const visibleStep = page
      .locator('[data-role="inline-step"]:visible')
      .first();
    await expect(visibleStep).toBeVisible({ timeout: 30_000 });
    await visibleStep.click();

    const modal = page.locator('[data-role="context-captured-modal"]:visible');
    await expect(modal).toBeVisible();
    await expect(modal.locator('[data-role="budget-bar"]')).toBeVisible();

    const sectionRows = modal.locator('[data-role="section-row"]');
    expect(await sectionRows.count()).toBeGreaterThan(1);

    // Mock LLM (LUCIDOS_MODEL=mock, see lib/e2e.sh) returns None for
    // usage, so the row may legitimately be absent.
    const usageRow = modal.locator('[data-role="usage-row"]');
    if ((await usageRow.count()) > 0) {
      await expect(usageRow).toContainText(/input/i);
    }
  });
});
