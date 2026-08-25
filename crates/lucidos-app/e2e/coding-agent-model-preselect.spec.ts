import { test, expect } from './fixtures';
import {
  navigateToApp, assertHealthy, newThread, pickComposeDestination,
  sendMessage, waitForCCToStart, waitForActionPanel, dismissCCSession,
  waitForActiveSession, waitAndClick, waitForVisibleElement, pickModelPair,
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
    await pickComposeDestination(page);

    await waitAndClick(page, '.commands-btn-active', undefined, 15_000);
    await waitAndClick(page, '.control-item', 'Model');
    // Two steps: the model, then one of its tiers. Only the tier reports.
    await pickModelPair(page, 'haiku');

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

    await waitAndClick(page, '.commands-btn-active', undefined, 15_000);
    await waitAndClick(page, '.control-item', 'Model');
    await waitForVisibleElement(page, '.control-option');

    // Haiku is the checked model. A step-1 row carries the model id alone.
    const currentOption = page.locator('.control-option-current:visible').first();
    await expect(currentOption).toBeVisible({ timeout: 5_000 });
    expect(await currentOption.getAttribute('data-value')).toBe('haiku');

    // Cleanup
    await page.keyboard.press('Escape');
    await dismissCCSession(page);
  });
});
