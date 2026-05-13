import { test, expect } from '@playwright/test';
import { execSync } from 'child_process';
import {
  navigateToApp, sendMessage, sendFollowUp, uniqueMessage,
  assertHealthy, switchToClaudeMode, newThread,
  waitForActionPanel, waitForCCToFinish, waitForCCToStart,
  countVisibleResponses, getLatestVisibleResponseText,
} from './helpers';
import { clearAllThreads } from './db-helpers';

/**
 * Reproduces the "stuck request after CC process exit" bug:
 *
 * 1. Start CC session → wait for idle (Done)
 * 2. Kill the CC child process (simulates natural process exit after timeout)
 * 3. Send follow-up message
 * 4. Verify the follow-up gets an actual response (not silently dropped)
 *
 * Bug: when the CC process dies while idle and the user sends a follow-up,
 * spawn_or_resume resumes with the stale session ID. The CC binary starts,
 * emits Init + empty Result, then exits immediately. The engine treats the
 * empty Result as a valid (empty) response — the user's message is silently
 * dropped with no status label or error.
 */
test.describe('CC resume after process exit', () => {
  test.beforeEach(async ({ page }) => {
    clearAllThreads();
    await assertHealthy(page);
  });

  test('follow-up after CC process dies produces a non-empty response', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);
    await switchToClaudeMode(page);

    // Step 1: Send initial CC message and wait for idle
    const msg1 = uniqueMessage('cc-resume-exit-1');
    await sendMessage(page, `Say exactly: "first ${msg1}" and nothing else. Do not create any files.`);
    await waitForActionPanel(page, 'Archive', 120_000);

    // Verify first response has content
    const firstResponseCount = await countVisibleResponses(page);
    expect(firstResponseCount).toBeGreaterThanOrEqual(1);

    // Step 2: Find and kill the CC child process for this thread.
    // The thread ID is in the URL or we can find it via DB.
    // Kill all idle claude processes — in e2e there should only be ours.
    try {
      // Kill claude processes that are children of the engine (not the engine itself).
      // The CC binary runs as a child process; killing it simulates natural timeout exit.
      execSync(
        `pgrep -f 'claude.*--resume\\|claude.*mcp' | xargs kill 2>/dev/null || true`,
        { encoding: 'utf-8', timeout: 5_000 },
      );
    } catch {
      // Process may already be gone — that's fine
    }

    // Give the engine a moment to notice the process exit
    await page.waitForTimeout(2_000);

    // Step 3: Send follow-up — this should trigger a fresh CC session, not a stale resume
    const msg2 = uniqueMessage('cc-resume-exit-2');
    await sendFollowUp(page, `Say exactly: "second ${msg2}" and nothing else. Do not create any files.`);

    // Step 4: Verify the follow-up is processed
    // The exchange should show "Requesting..." then "Working on it..." status labels
    // (Bug: no status label was shown — request was silently dropped)
    await waitForCCToStart(page, 120_000);

    await waitForCCToFinish(page, 120_000);

    // Verify the follow-up message produced an actual response (not empty)
    const responseCount = await countVisibleResponses(page);
    expect(responseCount).toBeGreaterThanOrEqual(2);

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
   *
   * Note: the existing "follow-up after CC process dies" test above tries
   * to kill claude processes, but its `pgrep -f 'claude.*--resume\\|claude.*mcp'`
   * pattern is broken (BRE `\\|` does not alternate in macOS pgrep's ERE),
   * so the kill is effectively a no-op. Both tests therefore exercise the
   * in-memory routing path; this one adds the content assertion the
   * non-empty test cannot give.
   */
  test('revival preserves conversation content (codeword round-trip)', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);
    await switchToClaudeMode(page);

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
    await switchToClaudeMode(page);

    const msg1 = uniqueMessage('cc-status-label-1');
    await sendMessage(page, `Say exactly: "first ${msg1}" and nothing else. Do not create any files.`);
    await waitForActionPanel(page, 'Archive', 120_000);

    // Kill CC process to simulate natural exit
    try {
      execSync(
        `pgrep -f 'claude.*--resume\\|claude.*mcp' | xargs kill 2>/dev/null || true`,
        { encoding: 'utf-8', timeout: 5_000 },
      );
    } catch {
      // Process may already be gone
    }
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
