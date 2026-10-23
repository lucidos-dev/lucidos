import { test, expect } from './fixtures';
import {
  navigateToApp, assertHealthy, newThread, pickComposeDestination,
  sendMessageCapturingThreadId, waitForCCToStart, waitForActionPanel, dismissCCSession,
  waitForActiveSession, openRungList, readRungRows, pickRung, currentRungValue,
} from './helpers';
import { clearAllThreads } from './db-helpers';

/**
 * Bug: when a user selects reasoning effort before starting a CC session,
 * the backend ignored the reasoning_effort field in the chat request. The CC process always used
 * the default effort from CC config files.
 *
 * This test picks the top ladder rung before starting a session. It checks
 * the commands API reflects that rung's effort after session start.
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

    // The last rung. A tap commits its model and its effort together.
    await openRungList(page);
    const rungs = await readRungRows(page);
    const picked = rungs[rungs.length - 1];
    expect(picked.effort).toBeTruthy();
    await pickRung(page, picked.value);

    const sentThreadId = await sendMessageCapturingThreadId(
      page, 'Say exactly: "reasoning-preselect-ok". Do not create any files.',
    );

    await waitForCCToStart(page, 60_000);

    // === Core bug assertion ===
    const cmdData = await waitForActiveSession(page, sentThreadId);
    expect(cmdData.current_reasoning_effort).toBe(picked.effort);

    await waitForActionPanel(page, 'Archive', 120_000);

    // The rung at that effort is the checked row once the session runs on it.
    await openRungList(page);
    expect(await currentRungValue(page)).toBe(picked.value);

    // Cleanup
    await page.keyboard.press('Escape');
    await dismissCCSession(page);
  });
});
