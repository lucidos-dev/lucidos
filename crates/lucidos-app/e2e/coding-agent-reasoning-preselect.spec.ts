import { test, expect } from './fixtures';
import {
  navigateToApp, assertHealthy, newThread, pickComposeDestination,
  sendMessage, waitForCCToStart, waitForActionPanel, dismissCCSession,
  waitForActiveSession, waitAndClick, waitForVisibleElement, pickModelPair,
  openCurrentModelTiers,
} from './helpers';
import { clearAllThreads } from './db-helpers';

/**
 * Bug: when a user selects reasoning effort before starting a CC session
 * (via the CC menu's "Next Session" settings), the backend ignored the
 * reasoning_effort field in the chat request. The CC process always used
 * the default effort from CC config files.
 *
 * This test selects "Max" reasoning effort via the CC menu before starting
 * a session and verifies the commands API reflects it after session start.
 */

test.describe('CC reasoning effort pre-session selection', () => {
  test.beforeEach(async ({ page }) => {
    clearAllThreads();
    await assertHealthy(page);
  });

  test('reasoning_effort in chat request is reflected in commands API after session start', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);
    await pickComposeDestination(page);

    await waitAndClick(page, '.commands-btn-active', undefined, 15_000);
    // One entry, not two: the tier is the picker's second step. With no
    // session and no pick, the model in force is Default.
    await waitAndClick(page, '.control-item', 'Model');
    await pickModelPair(page, 'default', 'max');

    let sentThreadId: string | null = null;
    page.on('request', (req) => {
      if (req.url().includes('/api/v1/chat/stream') && req.method() === 'POST') {
        try {
          const body = req.postDataJSON();
          sentThreadId = body?.thread_id ?? null;
        } catch { /* ignore */ }
      }
    });

    await sendMessage(page, 'Say exactly: "reasoning-preselect-ok". Do not create any files.');

    await waitForCCToStart(page, 60_000);
    expect(sentThreadId).toBeTruthy();

    // === Core bug assertion ===
    const cmdData = await waitForActiveSession(page, sentThreadId!);
    expect(cmdData.current_reasoning_effort).toBe('max');

    await waitForActionPanel(page, 'Archive', 120_000);

    await waitAndClick(page, '.commands-btn-active', undefined, 15_000);
    await waitAndClick(page, '.control-item', 'Model');
    await waitForVisibleElement(page, '.control-option');

    // The session's own model at Max is now the checked pair, one step in.
    await openCurrentModelTiers(page);
    const currentTier = page.locator('.control-option-current:visible').first();
    await expect(currentTier).toBeVisible({ timeout: 5_000 });
    expect(await currentTier.getAttribute('data-value')).toMatch(/\|max$/);

    // Cleanup
    await page.keyboard.press('Escape');
    await dismissCCSession(page);
  });
});
