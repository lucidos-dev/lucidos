import { test, expect } from './fixtures';
import {
  navigateToApp,
  newThread,
  sendMessage,
  waitForResponse,
  assertHealthy,
  revealSteps,
} from './helpers';

test.describe('LLM Context Viewer two-layer grouping', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('groups sections by API role with inner tier nesting', async ({ page }) => {
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

    await expect(modal.locator('.context-role-label', { hasText: 'System role' }))
      .toBeVisible();
    await expect(modal.locator('.context-role-label', { hasText: 'User message (this turn)' }))
      .toBeVisible();

    await expect(
      modal.locator('.context-inner-label', { hasText: 'The request' })
    ).toBeVisible();

    const innerGroupCount = await modal.locator('.context-inner-group').count();
    expect(innerGroupCount).toBeGreaterThanOrEqual(1);

    const nestedSections = modal.locator('.context-inner-group [data-role="section-row"]');
    expect(await nestedSections.count()).toBeGreaterThanOrEqual(1);
  });
});
