import { test, expect } from '@playwright/test';
import {
  navigateToApp, assertHealthy, newThread, switchToClaudeMode,
  sendMessage, waitForCCToStart, waitForActionPanel, dismissCCSession,
  waitForActiveSession, waitAndClick, waitForVisibleElement,
} from './helpers';
import { clearAllThreads } from './db-helpers';

/**
 * Bug: when a user selects a model before starting a CC session, the backend's
 * ClaudeCodeSession.current_model was not set from cc_model. The commands API
 * returned stale/null current_model until the CC process Init event arrived.
 *
 * This test selects "Haiku" via the CC menu before starting a session and
 * verifies the commands API and UI reflect it after session start.
 */

test.describe('CC model pre-session selection', () => {
  test.beforeEach(async ({ page }) => {
    clearAllThreads();
    await assertHealthy(page);
  });

  test('cc_model in chat request is reflected in commands API after session start', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);
    await switchToClaudeMode(page);

    await waitAndClick(page, '.cc-commands-btn-active', undefined, 15_000);
    await waitAndClick(page, '.cc-control-item', 'Model');
    await waitAndClick(page, '.cc-control-option', 'Haiku');

    let sentThreadId: string | null = null;
    page.on('request', (req) => {
      if (req.url().includes('/api/v1/chat/stream') && req.method() === 'POST') {
        try {
          const body = req.postDataJSON();
          sentThreadId = body?.thread_id ?? null;
        } catch { /* ignore */ }
      }
    });

    await sendMessage(page, 'Say exactly: "model-preselect-ok". Do not create any files.');

    await waitForCCToStart(page, 60_000);
    expect(sentThreadId).toBeTruthy();

    // === Core bug assertion ===
    const cmdData = await waitForActiveSession(page, sentThreadId!);
    expect(cmdData.current_model).toBe('haiku');

    await waitForActionPanel(page, 'Archive', 120_000);

    await waitAndClick(page, '.cc-commands-btn-active', undefined, 15_000);
    await waitAndClick(page, '.cc-control-item', 'Model');
    await waitForVisibleElement(page, '.cc-control-option');

    // Haiku should be marked as current (has cc-control-option-current class)
    const currentOption = page.locator('.cc-control-option-current:visible').first();
    await expect(currentOption).toBeVisible({ timeout: 5_000 });
    const currentLabel = await currentOption.locator('.cc-control-option-label').textContent();
    expect((currentLabel ?? '').replace(/✓/g, '').trim()).toBe('Haiku 4.5');

    // Cleanup
    await page.keyboard.press('Escape');
    await dismissCCSession(page);
  });
});
