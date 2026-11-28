import { test, expect } from './fixtures';
import {
  navigateToApp, assertHealthy, newThread, pickComposeDestination,
  sendMessageCapturingThreadId, waitForCCToStart, waitForActionPanel, dismissCCSession,
  waitForActiveSession, openRungList, readRungRows, pickRung, expectPickRanOrSubstituted,
  modelSubstitutions,
} from './helpers';
import { clearAllThreads } from './db-helpers';


test.describe('CC model pre-session selection', () => {
  test.beforeEach(async ({ page }) => {
    clearAllThreads();
    await assertHealthy(page);
  });

  test('cc_model in chat request is reflected in commands API after session start', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);
    await pickComposeDestination(page);

    await openRungList(page);
    const picked = (await readRungRows(page)).find((r) => !r.current);
    expect(picked).toBeTruthy();
    await pickRung(page, picked!.value);

    const sentThreadId = await sendMessageCapturingThreadId(
      page, 'Say exactly: "model-preselect-ok". Do not create any files.',
    );

    await waitForCCToStart(page, 60_000);

    const cmdData = await waitForActiveSession(page, sentThreadId);
    await expect(() => {
      const served = modelSubstitutions(sentThreadId).map((s) => s.served);
      expect([picked!.model, ...served]).toContain(cmdData.current_model);
    }).toPass({ timeout: 5_000 });
    expect(cmdData.current_reasoning_effort).toBe(picked!.effort);

    await waitForActionPanel(page, 'Archive', 120_000);

    await expectPickRanOrSubstituted(page, sentThreadId, picked!);

    await dismissCCSession(page);
  });
});
