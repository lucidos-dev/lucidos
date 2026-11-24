import { test, expect } from './fixtures';
import {
  navigateToApp, assertHealthy, newThread, pickComposeDestination,
  sendMessageCapturingThreadId, waitForCCToStart, waitForActionPanel, dismissCCSession,
  waitForActiveSession, openRungList, readRungRows, pickRung, expectPickRanOrSubstituted,
} from './helpers';
import { clearAllThreads } from './db-helpers';


test.describe('CC reasoning effort pre-session selection', () => {
  test.beforeEach(async ({ page }) => {
    clearAllThreads();
    await assertHealthy(page);
  });

  test('reasoning_effort in chat request is reflected in commands API after session start', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);
    await pickComposeDestination(page);

    await openRungList(page);
    const rungs = await readRungRows(page);
    const picked = rungs[rungs.length - 1];
    expect(picked.effort).toBeTruthy();
    await pickRung(page, picked.value);

    const sentThreadId = await sendMessageCapturingThreadId(
      page, 'Say exactly: "reasoning-preselect-ok". Do not create any files.',
    );

    await waitForCCToStart(page, 60_000);

    const cmdData = await waitForActiveSession(page, sentThreadId);
    expect(cmdData.current_reasoning_effort).toBe(picked.effort);

    await waitForActionPanel(page, 'Archive', 120_000);

    await expectPickRanOrSubstituted(page, sentThreadId, picked);

    await dismissCCSession(page);
  });
});
