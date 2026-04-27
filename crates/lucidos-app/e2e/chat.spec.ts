import { test, expect } from '@playwright/test';
import {
  navigateToApp, sendMessage, sendFollowUp, waitForResponse, uniqueMessage,
  assertHealthy, openThreadDrawer, userMessageBody, switchToClaudeMode, newThread,
  waitForCCToFinish, waitForCCToStart,
} from './helpers';

test.describe('Chat - send and receive messages', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('send a message and see a response', async ({ page }) => {
    await navigateToApp(page);

    const msg = uniqueMessage('chat-basic');
    await sendMessage(page, `Say exactly: "hello ${msg}"`);

    // User message should appear in the thread (use first visible one)
    await expect(userMessageBody(page)).toContainText(msg, { timeout: 10_000 });

    // Wait for the LLM response to finish
    const response = await waitForResponse(page);
    const responseText = await response.textContent();
    expect(responseText!.trim().length).toBeGreaterThan(0);
  });

  test('thread appears in the sidebar after sending a message', async ({ page }) => {
    await navigateToApp(page);

    const msg = uniqueMessage('sidebar-thread');
    await sendMessage(page, `Say exactly: "acknowledged ${msg}"`);
    await waitForResponse(page);

    // Open the thread drawer
    await openThreadDrawer(page);

    // A thread row should appear in the sidebar
    const threadRows = page.locator('.thread-row:visible');
    await expect(threadRows.first()).toBeVisible({ timeout: 15_000 });

    // The focused thread should be highlighted
    const focusedRow = page.locator('.thread-row-focused:visible').first();
    await expect(focusedRow).toBeVisible();
  });

  test('response has non-empty content', async ({ page }) => {
    await navigateToApp(page);

    await sendMessage(page, `What is 2 + 2? Reply with just the number.`);
    const response = await waitForResponse(page);

    const text = await response.textContent();
    expect(text!.trim().length).toBeGreaterThan(0);
  });

  /**
   * Pin: WITHIN a single CC turn, a follow-up user message must reach CC's
   * stdin via the live msg_rx → agent_input_tx pipe. Phase 2 of the resume
   * architecture made every CC subprocess exit on idle (between turns CC is
   * dead). This test guards the within-turn path so an over-aggressive
   * cancel-on-idle (or accidental removal of the msg_rx pipe) cannot
   * silently regress.
   *
   * Failure modes this catches:
   * - Final response references only the codeword (no bash output): CC
   *   restarted mid-turn and lost the original task — Phase 2 cancellation
   *   leaked into the within-turn path.
   * - Final response references only the bash output (no codeword): msg2
   *   was dropped — the within-turn stdin pipe is broken.
   *
   * The slow bash task (15x echo + sleep 2 = ~30s) gives a wide window for
   * msg2 to land mid-turn. Detection of "mid-tool-call" polls the events
   * API for CodingAgentToolCalled — deterministic, not a fixed timer.
   */
  test('mid-turn user message reaches CC during active tool calls', async ({ page }) => {
    test.setTimeout(180_000);

    await navigateToApp(page);
    await newThread(page);
    await switchToClaudeMode(page);

    // msg1: long-running bash task — the same shape as cc-cancel's
    // BUSY_BASH_PROMPT (15 iterations of sleep 2 ≈ 30s). The "widgetN" tokens
    // are easy to assert on, and the wide window absorbs scheduling jitter
    // so msg2 reliably lands while CC is still inside the bash tool call.
    const slowTask =
      `Run this exact bash command and include the full output verbatim in your final answer: ` +
      `bash -c 'for i in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15; do echo widget$i; sleep 2; done'`;
    await sendMessage(page, slowTask);

    // Wait for CC to begin (status label transitions to Working/Requesting).
    // This proves the exchange is live and the focused thread ID is settled
    // in localStorage — required for the events-API poll below.
    await waitForCCToStart(page, 60_000);

    // Pull the focused thread ID from the app's localStorage. The store
    // writes 'lucidos-focused-thread' as soon as the thread is created
    // (before the first message is sent), so by the time CC has started
    // working it is guaranteed to be present.
    const threadId = await page.evaluate(() => localStorage.getItem('lucidos-focused-thread'));
    expect(threadId, 'focused thread id not in localStorage').toBeTruthy();

    // Deterministic mid-tool-call signal: poll the events API until a
    // CodingAgentToolCalled event lands for this thread. Steps are hidden
    // by default in the UI, so we cannot rely on .inline-step rendering —
    // the event store is the source of truth.
    await expect.poll(
      async () => {
        const resp = await page.request.get(`/api/threads/${threadId}/events`);
        if (!resp.ok()) return false;
        const body = await resp.json();
        const events: Array<{ event_type?: string }> = Array.isArray(body)
          ? body
          : (body.events ?? []);
        return events.some(e => e.event_type === 'CodingAgentToolCalled');
      },
      { intervals: [500, 1000, 2000], timeout: 60_000 },
    ).toBe(true);

    // msg2: arrives WHILE the bash sleep loop is still running. If Phase 2
    // killed the process on idle (it shouldn't, mid-turn) or the msg_rx pipe
    // is gone, this message will not reach CC.
    await sendFollowUp(page, `Also remember the codeword "horseshoe" and include it in your final answer.`);

    // Wait for the whole turn (original task + mid-turn add-on) to settle.
    await waitForCCToFinish(page, 120_000);

    // Pull all visible response text — CC may emit multiple response chunks
    // across the turn; the within-turn promise is that the user sees both
    // pieces of work in their answer, regardless of which chunk holds which.
    const responseText = await page.evaluate(() => {
      const els = document.querySelectorAll('.response-content');
      const visible = Array.from(els).filter(el => {
        const rect = el.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0 && (el.textContent ?? '').trim().length > 0;
      });
      return visible.map(el => el.textContent ?? '').join('\n');
    });

    const lower = responseText.toLowerCase();
    expect(
      lower,
      `Response did not reference the original bash task. If CC restarted ` +
        `mid-turn (Phase 2 cancellation leaking into the within-turn path), ` +
        `the bash output is lost and only the codeword survives. ` +
        `Response: ${JSON.stringify(responseText)}`,
    ).toContain('widget');
    expect(
      lower,
      `Response did not reference the mid-turn codeword. If the msg_rx → ` +
        `agent_input_tx pipe is broken, msg2 never reaches CC's stdin and ` +
        `the codeword is silently dropped. ` +
        `Response: ${JSON.stringify(responseText)}`,
    ).toContain('horseshoe');
  });
});
