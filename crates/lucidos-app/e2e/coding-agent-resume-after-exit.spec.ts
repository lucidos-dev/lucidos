import { test, expect } from './fixtures';
import {
  navigateToApp, sendMessage, sendFollowUp, uniqueMessage,
  assertHealthy, pickComposeDestination, newThread,
  waitForActionPanel, waitForCCToFinish, waitForCCToStart,
  countVisibleResponses, getLatestVisibleResponseText,
} from './helpers';
import { clearAllThreads } from './db-helpers';

/**
 * Guards the follow-up path a "stuck request after CC process exit" report
 * came from: a follow-up must produce a real response with a status label,
 * never be silently dropped.
 *
 * The originating bug is that `spawn_or_resume` resumed with a stale session
 * id, whose CC binary emits Init plus an empty Result and exits. The engine
 * read that empty Result as a valid response.
 *
 * **These tests do NOT kill the CC subprocess.** They used to try, through a
 * `pgrep -f 'claude…' | xargs kill` whose pattern never matched, so the step
 * was inert. That shape must not come back. It matches every concurrent
 * workspace's agent on this host, so it would destroy other people's sessions
 * the day someone fixed the pattern. A real reproduction has to kill the
 * engine's own child pid. Until then these cover the live in-memory routing
 * path, and the codeword test below adds the content assertion the non-empty
 * one cannot give.
 */
test.describe('CC resume after process exit', () => {
  test.beforeEach(async ({ page }) => {
    clearAllThreads();
    await assertHealthy(page);
  });

  test('a follow-up on an idle CC thread produces a non-empty response', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);
    await pickComposeDestination(page);

    // Step 1: Send initial CC message and wait for idle
    const msg1 = uniqueMessage('cc-resume-exit-1');
    await sendMessage(page, `Say exactly: "first ${msg1}" and nothing else. Do not create any files.`);
    await waitForActionPanel(page, 'Archive', 120_000);

    // Verify first response has content
    const firstResponseCount = await countVisibleResponses(page);
    expect(firstResponseCount).toBeGreaterThanOrEqual(1);

    // Let the idle session settle before the follow-up, so the send lands on a
    // settled thread rather than one still writing its Done state.
    await page.waitForTimeout(2_000);

    // Step 2: Send the follow-up
    const msg2 = uniqueMessage('cc-resume-exit-2');
    await sendFollowUp(page, `Say exactly: "second ${msg2}" and nothing else. Do not create any files.`);

    // Step 3: Verify the follow-up is processed
    // The exchange should show "Requesting..." then "Working on it..." status labels
    // (Bug: no status label was shown — request was silently dropped)
    await waitForCCToStart(page, 120_000);

    await waitForCCToFinish(page, 120_000);

    // Verify the follow-up message produced an actual response (not empty).
    // Poll: CC response text can still be rendering right after
    // waitForCCToFinish (which gates on the status label, not content).
    await expect
      .poll(() => countVisibleResponses(page), { timeout: 30_000 })
      .toBeGreaterThanOrEqual(2);

    // Verify the second response contains our marker
    const found = await page.evaluate((marker) => {
      const els = document.querySelectorAll('.response-content');
      return Array.from(els).some(el => {
        const rect = el.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0 && (el.textContent ?? '').includes(marker);
      });
    }, msg2);
    expect(found).toBe(true);
  });

  /**
   * Stronger assertion than the non-empty check above: prove the second
   * response references content from the first turn. Without this, a CC
   * regression that loses conversation memory ("I don't recall what you
   * asked") would slip past the non-empty check and look like a passing test.
   *
   * Why "pineapple": specific enough that an amnesiac CC has effectively
   * zero chance of producing it. A "non-empty" assertion is satisfied by
   * any answer; a "contains pineapple" assertion is only satisfied by
   * genuine memory of the prior turn.
   *
   * Why no kill step: when CC's in-memory agent_session is alive between
   * turns, chat/process.rs routes the follow-up through msg_tx — same live
   * CC process, conversation memory naturally preserved. When the in-memory
   * session is gone (subprocess died, engine restart), the slow path calls
   * `run_direct_agent` with a resume sid resolved from the event store
   * (the fix in 76529c04). Either path must preserve the codeword. The
   * unit test in resume.rs already exercises the resolver mechanics; this
   * test guards the user-visible promise that conversation context survives.
   */
  test('revival preserves conversation content (codeword round-trip)', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);
    await pickComposeDestination(page);

    // Turn 1: tell CC to remember a codeword. Keep the response short and
    // file-free so the response stays a single recognizable word.
    await sendMessage(
      page,
      'Remember the codeword: pineapple. Just acknowledge with one word. Do not create any files.',
    );
    await waitForCCToStart(page, 60_000);
    await waitForCCToFinish(page, 120_000);

    const firstResponseCount = await countVisibleResponses(page);
    expect(firstResponseCount).toBeGreaterThanOrEqual(1);

    // Turn 2: ask CC to recall the codeword.
    await sendFollowUp(
      page,
      'What was the codeword I asked you to remember? Reply with just the word.',
    );

    await waitForCCToStart(page, 120_000);
    await waitForCCToFinish(page, 120_000);

    // The status label can flip Done before the final response chunk reaches
    // the DOM (msg_tx fast-path between turns can also flash a transient
    // non-Working state), so poll until the codeword appears in the latest
    // response. CC sometimes capitalizes a single-word answer — lowercase
    // before matching.
    await expect(async () => {
      const latest = await getLatestVisibleResponseText(page);
      expect(latest.toLowerCase()).toContain('pineapple');
    }).toPass({ timeout: 30_000, intervals: [500, 1000, 2000] });
  });

  test('status label shows Requesting during CC follow-up', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);
    await pickComposeDestination(page);

    const msg1 = uniqueMessage('cc-status-label-1');
    await sendMessage(page, `Say exactly: "first ${msg1}" and nothing else. Do not create any files.`);
    await waitForActionPanel(page, 'Archive', 120_000);

    // Let the idle session settle before the follow-up, so the send lands on a
    // settled thread rather than one still writing its Done state.
    await page.waitForTimeout(2_000);

    // Send follow-up and immediately check for status label
    const msg2 = uniqueMessage('cc-status-label-2');
    await sendFollowUp(page, `Say exactly: "second ${msg2}" and nothing else. Do not create any files.`);

    // The exchange should show a "Requesting" or "Working" status label
    // Bug: no status label was shown at all
    const hasStatusLabel = await page.waitForFunction(() => {
      const labels = document.querySelectorAll('.exchange-status-label');
      return Array.from(labels).some(el => {
        const rect = el.getBoundingClientRect();
        if (rect.width === 0 || rect.height === 0) return false;
        const text = el.textContent ?? '';
        return text.includes('Requesting') || text.includes('Working');
      });
    }, undefined, { timeout: 30_000 }).then(() => true).catch(() => false);

    expect(hasStatusLabel).toBe(true);

    await waitForCCToFinish(page, 120_000);
  });
});
