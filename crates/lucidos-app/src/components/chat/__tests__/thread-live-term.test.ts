import { describe, it, expect } from 'vitest';
import { exchangeMarksThreadLive } from '../ChatExchange';
import { exchangeStatus } from '../../../store/thread-events';
import { makeExchange, step } from '../../../store/__tests__/fixtures';

/**
 * **The follow's live term describes the THREAD, not the last turn's rendering.**
 *
 * `scrollState` retires a standing follow when the reader scrolls away, and only
 * while the agent is live: fleeing a reply in flight means "stop dragging me",
 * scrolling an idle thread is browsing and must cost the reader nothing. So the
 * term has to be true exactly when something is running.
 *
 * It was read off the last exchange's status alone, and that is a rendering
 * verdict about one turn whose final fallthrough is `'pending'` ("a coding-agent
 * turn with no step yet"), which `isActive` counts as live. A stepless SYSTEM
 * boundary lands on that line too: `ChangeApplied` opens an exchange of its own,
 * so a coding-agent thread whose change was applied ends in an exchange with no
 * steps and no terminal. Nothing painted the phantom "Requesting" (a bare
 * boundary draws no response row), so the wrong status was invisible until the
 * live term started reading it, and then one scroll retired the follow on an
 * idle thread. Reported 2026-08-10 against a real thread whose last event was
 * `ChangeApplied`.
 *
 * The fix asks the thread projection too. These tests run the REAL
 * `exchangeStatus` rather than asserting a status literal, so they still hold if
 * that function's fallthrough is ever changed.
 */
describe("the follow's live term", () => {
  const CHANGE_APPLIED = { type: 'ChangeApplied', change_id: 'c1' } as const;

  it('is false for a stepless ChangeApplied boundary closing an idle coding-agent thread', () => {
    const boundary = makeExchange(CHANGE_APPLIED, []);
    const status = exchangeStatus(boundary, '', true, false, true, true, false);
    // The rendering verdict is the generous one, which is what made this a bug.
    expect(status).toBe('pending');
    expect(exchangeMarksThreadLive(true, status, true)).toBe(false);
  });

  it('is true while the coding agent is actually working', () => {
    const working = makeExchange({ type: 'MessageReceived', text: 'go' }, [
      step(1, { type: 'CodingAgentToolCalled', name: 'Bash', args: {} }),
    ]);
    const status = exchangeStatus(working, '', true, false, true, false, false);
    expect(status).toBe('coding-agent-working');
    expect(exchangeMarksThreadLive(true, status, false)).toBe(true);
  });

  it('is true in the gap between a send and its first step, which the projection has not settled', () => {
    // The case `exchangeStatus`'s 'pending' fallthrough exists for: a real send
    // whose coding-agent session is still spawning. The projection says running,
    // so the term must not have narrowed this away.
    const justSent = makeExchange({ type: 'MessageReceived', text: 'go' }, []);
    const status = exchangeStatus(justSent, '', true, false, true, false, false);
    expect(status).toBe('pending');
    expect(exchangeMarksThreadLive(true, status, false)).toBe(true);
  });

  it('is true for a turn that armed an event wait and worked on', () => {
    // The agent's watch is armed mid-turn and the turn carries on, so the
    // thread is live and a scroll must retire the follow. The park used to
    // speak for the whole turn, which read "Done ✓" over a working agent.
    const worked = makeExchange({ type: 'MessageReceived', text: 'run the suite' }, [
      step(1, { type: 'CodingAgentToolCalled', name: 'Bash', args: {} }),
      step(2, {
        type: 'EventWaitStarted', wait_id: 'w1', tool_use_id: 't1',
        on: [{ event_type: 'E2ELockReleased' }], reason: 'the lock',
        expires_at: '2026-08-06T12:00:00Z', watermark: 10,
      }),
      step(3, { type: 'CodingAgentToolCalled', name: 'Bash', args: {} }),
    ]);
    const status = exchangeStatus(worked, '', true, false, true, false, false);
    expect(status).toBe('coding-agent-working');
    expect(exchangeMarksThreadLive(true, status, false)).toBe(true);
  });

  it('is false for a thread parked on a question, where nothing is being appended', () => {
    const asked = makeExchange({
      type: 'UserQuestionAsked', tool_use_id: 't1', cc_session_id: 's1', question: 'which?',
    }, []);
    const status = exchangeStatus(asked, '', true, false, true, true, true);
    expect(exchangeMarksThreadLive(true, status, true)).toBe(false);
  });

  it('is false for any exchange that is not the last one', () => {
    const working = makeExchange({ type: 'MessageReceived', text: 'go' }, [
      step(1, { type: 'CodingAgentToolCalled', name: 'Bash', args: {} }),
    ]);
    const status = exchangeStatus(working, '', true, false, true, false, false);
    expect(exchangeMarksThreadLive(false, status, false)).toBe(false);
  });
});
