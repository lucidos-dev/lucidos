import { test, expect } from './fixtures';
import {
  navigateToApp, assertHealthy, newThread, pickComposeDestination,
  sendMessageCapturingThreadId, waitForCCToStart, waitForActionPanel, dismissCCSession,
  waitForActiveSession, openRungList, readRungRows, pickRung, currentRungValue,
} from './helpers';
import { clearAllThreads } from './db-helpers';

/**
 * Bug: when a user selects a model before starting a CC session, the backend's
 * ClaudeCodeSession.current_model was not set from cc_model. The commands API
 * returned stale/null current_model until the CC process Init event arrived.
 *
 * This test picks a ladder rung from the prompt bar's rung control before
 * starting a session. It checks the commands API and the control reflect the
 * rung's model and effort after session start.
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

    // The first rung that is not the checked row. A tap commits its model and
    // its effort together.
    await openRungList(page);
    const picked = (await readRungRows(page)).find((r) => !r.current);
    expect(picked).toBeTruthy();
    await pickRung(page, picked!.value);

    const sentThreadId = await sendMessageCapturingThreadId(
      page, 'Say exactly: "model-preselect-ok". Do not create any files.',
    );

    await waitForCCToStart(page, 60_000);

    // === Core bug assertion ===
    const cmdData = await waitForActiveSession(page, sentThreadId);
    expect(cmdData.current_model).toBe(picked!.model);
    expect(cmdData.current_reasoning_effort).toBe(picked!.effort);

    await waitForActionPanel(page, 'Archive', 120_000);

    // The rung is the checked row once the session runs on it.
    await openRungList(page);
    expect(await currentRungValue(page)).toBe(picked!.value);

    // Cleanup
    await page.keyboard.press('Escape');
    await dismissCCSession(page);
  });
});
