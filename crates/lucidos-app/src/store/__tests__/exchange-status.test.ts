import { describe, it, expect } from 'vitest';
import {
  exchangeStatus,
  type Exchange,
  type StoredEvent,
  type SequencedEvent,
} from '../thread-events';

/** Helper to build an Exchange from a user event + step events. */
function makeExchange(
  userEvent: StoredEvent,
  steps: Array<{ seq: number; event: StoredEvent }> = [],
): Exchange {
  return {
    userEvent,
    userSeq: 0,
    steps,
  };
}

function msg(text = 'hi'): StoredEvent {
  return { type: 'MessageReceived', text };
}

function step(seq: number, event: StoredEvent): SequencedEvent {
  return { seq, event };
}

describe('exchangeStatus — full branch coverage', () => {
  it('returns "error" when ResponseFailed is present', () => {
    const exchange = makeExchange(msg(), [
      step(1, { type: 'ToolCalled', name: 'x', args: {} }),
      step(2, { type: 'ResponseFailed', error: 'timeout' }),
    ]);
    expect(exchangeStatus(exchange, '', true)).toBe('error');
    expect(exchangeStatus(exchange, '', false)).toBe('error');
  });

  it('returns "canceled" when ResponseCanceled is present (and not failed)', () => {
    const exchange = makeExchange(msg(), [
      step(1, { type: 'TextStreamed', text: 'partial' }),
      step(2, { type: 'ResponseCanceled' }),
    ]);
    expect(exchangeStatus(exchange, '', true)).toBe('canceled');
    expect(exchangeStatus(exchange, '', false)).toBe('canceled');
  });

  it('returns "done" when CC is idle (WaitingBanner handles session state)', () => {
    const exchange = makeExchange(msg(), [
      step(1, { type: 'SessionStarted', session_id: 's1' }),
      step(2, { type: 'CodingAgentToolCalled', name: 'edit', args: {} }),
      step(3, { type: 'CodingAgentIdled' }),
    ]);
    expect(exchangeStatus(exchange, '', true)).toBe('done');
  });

  it('returns "interrupted" for non-last CC exchange mid-work (no idle, no complete)', () => {
    const exchange = makeExchange(msg(), [
      step(1, { type: 'SessionStarted', session_id: 's1' }),
      step(2, { type: 'CodingAgentToolCalled', name: 'edit', args: {} }),
    ]);
    expect(exchangeStatus(exchange, '', false)).toBe('interrupted');
  });

  it('returns "done" (not interrupted) for non-last CC exchange that went idle', () => {
    const exchange = makeExchange(msg(), [
      step(1, { type: 'SessionStarted', session_id: 's1' }),
      step(2, { type: 'CodingAgentToolCalled', name: 'edit', args: {} }),
      step(3, { type: 'CodingAgentIdled' }),
    ]);
    // isLast=false, but CC went idle — normal completion, not interrupted
    expect(exchangeStatus(exchange, '', false)).toBe('done');
  });

  it('returns "done" when isComplete=true (ResponseGenerated)', () => {
    const exchange = makeExchange(msg(), [
      step(1, { type: 'TextStreamed', text: 'hello' }),
      step(2, { type: 'ResponseGenerated' }),
    ]);
    expect(exchangeStatus(exchange, '', true)).toBe('done');
    expect(exchangeStatus(exchange, '', false)).toBe('done');
  });

  it('returns "aborted" when SessionEnded(shutdown) without response or idle', () => {
    // Only `shutdown`/`panic` reasons are system aborts — other reasons (and
    // missing reason from legacy rows) are normal lifecycle ends.
    const exchange = makeExchange(msg(), [
      step(1, { type: 'SessionStarted', session_id: 's1' }),
      step(2, { type: 'SessionEnded', reason: 'shutdown' }),
    ]);
    expect(exchangeStatus(exchange, '', true)).toBe('aborted');
    expect(exchangeStatus(exchange, '', false)).toBe('aborted');
  });

  it('returns "done" for CC exchange that idled then session ended', () => {
    const exchange = makeExchange(msg(), [
      step(1, { type: 'SessionStarted', session_id: 's1' }),
      step(2, { type: 'CodingAgentToolCalled', name: 'bash', args: {} }),
      step(3, { type: 'CodingAgentIdled', has_changes: false }),
      step(4, { type: 'SessionEnded' }),
    ]);
    expect(exchangeStatus(exchange, '', true)).toBe('done');
  });

  it('returns "aborted" for chat exchange with SessionEnded(shutdown) but no ResponseGenerated', () => {
    const exchange = makeExchange(msg(), [
      step(1, { type: 'SessionEnded', reason: 'shutdown' }),
    ]);
    expect(exchangeStatus(exchange, '', true)).toBe('aborted');
  });

  it('returns "done" when shutdown happens after CC completed (work was done)', () => {
    // Engine shutdown after CC completed work: ResponseGenerated + CodingAgentIdled
    // were emitted before SessionEnded(shutdown). The user's work was done —
    // the shutdown doesn't undo that.
    const exchange = makeExchange(msg(), [
      step(1, { type: 'SessionStarted', session_id: 's1' }),
      step(2, { type: 'CodingAgentToolCalled', name: 'bash', args: {} }),
      step(3, { type: 'ResponseGenerated', text: 'partial' }),
      step(4, { type: 'CodingAgentIdled', has_changes: false }),
      step(5, { type: 'SessionEnded', reason: 'shutdown' } as StoredEvent),
    ]);
    expect(exchangeStatus(exchange, '', true)).toBe('done');
    expect(exchangeStatus(exchange, '', false)).toBe('done');
  });

  it('returns "aborted" when shutdown happens mid-work (CC never completed)', () => {
    // Engine shutdown while CC was actively working — no ResponseGenerated or
    // CodingAgentIdled before the shutdown. The work was genuinely interrupted.
    const exchange = makeExchange(msg(), [
      step(1, { type: 'SessionStarted', session_id: 's1' }),
      step(2, { type: 'CodingAgentToolCalled', name: 'bash', args: {} }),
      step(3, { type: 'SessionEnded', reason: 'shutdown' } as StoredEvent),
    ]);
    expect(exchangeStatus(exchange, '', true)).toBe('aborted');
    expect(exchangeStatus(exchange, '', false)).toBe('aborted');
  });

  it('returns "done" for non-last non-CC exchange (even without completion event)', () => {
    const exchange = makeExchange(msg(), [
      step(1, { type: 'TextStreamed', text: 'partial' }),
    ]);
    // !isLast && !isCC => 'done' (line: if (isComplete || !isLast) return 'done')
    expect(exchangeStatus(exchange, '', false)).toBe('done');
  });

  it('returns "cc-working" for last CC exchange without completion or idle', () => {
    const exchange = makeExchange(msg(), [
      step(1, { type: 'SessionStarted', session_id: 's1' }),
      step(2, { type: 'CodingAgentToolCalled', name: 'bash', args: {} }),
    ]);
    expect(exchangeStatus(exchange, '', true)).toBe('cc-working');
  });

  it('returns "streaming" when streamingBuffer has content', () => {
    const exchange = makeExchange(msg());
    expect(exchangeStatus(exchange, 'partial text...', true)).toBe('streaming');
  });

  it('returns "streaming" when responseText exists but no completion event (buffer race)', () => {
    // This happens when persisted TextStreamed events clear the streaming buffer
    // but the response isn't complete yet (no ResponseGenerated). The exchange
    // should show "streaming"/"Working", not "Done".
    const exchange = makeExchange(msg(), [
      step(1, { type: 'TextStreamed', text: 'partial response so far' }),
    ]);
    expect(exchangeStatus(exchange, '', true)).toBe('streaming');
  });

  it('returns "done" when responseText exists WITH completion event', () => {
    const exchange = makeExchange(msg(), [
      step(1, { type: 'TextStreamed', text: 'full response' }),
      step(2, { type: 'ResponseGenerated' }),
    ]);
    expect(exchangeStatus(exchange, '', true)).toBe('done');
  });

  it('returns "streaming" when event has steps but no completion (backend handles crash detection)', () => {
    const oldDate = new Date(Date.now() - 120_000).toISOString();
    const exchange = makeExchange(
      { type: 'MessageReceived', text: 'hi', created: oldDate },
      [step(1, { type: 'ToolCalled', name: 'x', args: {}, created: oldDate })],
    );
    // No responseText, no streamingBuffer — but the backend emits ResponseAborted
    // on crash/restart, so without a terminal event this has a live process.
    // Falls through to the steps/events check → streaming.
    expect(exchangeStatus(exchange, '', true)).toBe('streaming');
  });

  it('returns "streaming" when old events exist (long-running tool call)', () => {
    const oldDate = new Date(Date.now() - 120_000).toISOString();
    const exchange = makeExchange(
      { type: 'MessageReceived', text: 'hi', created: oldDate },
      [step(1, { type: 'ToolCalled', name: 'edit_file', args: {}, created: oldDate })],
    );
    // No stale guard — backend emits ResponseAborted on crash/shutdown.
    // Steps without completion → streaming.
    expect(exchangeStatus(exchange, '', true, false, false)).toBe('streaming');
  });

  it('returns "streaming" when steps/events exist but not stale', () => {
    const recentDate = new Date().toISOString();
    const exchange = makeExchange(
      { type: 'MessageReceived', text: 'hi', created: recentDate },
      [step(1, { type: 'ToolCalled', name: 'x', args: {}, created: recentDate })],
    );
    expect(exchangeStatus(exchange, '', true)).toBe('streaming');
  });

  it('returns "pending" when nothing is available (no steps, no text, no buffer)', () => {
    const exchange = makeExchange(msg());
    expect(exchangeStatus(exchange, '', true)).toBe('pending');
  });

  it('failed takes priority over canceled', () => {
    const exchange = makeExchange(msg(), [
      step(1, { type: 'ResponseCanceled' }),
      step(2, { type: 'ResponseFailed', error: 'boom' }),
    ]);
    expect(exchangeStatus(exchange, '', true)).toBe('error');
  });

  it('canceled takes priority over idle done (ResponseCanceled after CodingAgentIdled)', () => {
    const exchange = makeExchange(msg(), [
      step(1, { type: 'SessionStarted', session_id: 's1' }),
      step(2, { type: 'CodingAgentIdled' }),
      step(3, { type: 'ResponseCanceled' }),
    ]);
    expect(exchangeStatus(exchange, '', true)).toBe('canceled');
  });

  it('canceled takes priority over idle done (ResponseCanceled before CodingAgentIdled — interrupt flow)', () => {
    // When user hits stop on a CC follow-up, the backend emits ResponseCanceled
    // first (marking the exchange as canceled), then CodingAgentIdled (keeping
    // the session alive). The exchange must show "canceled", not "done".
    const exchange = makeExchange(msg(), [
      step(1, { type: 'CodingAgentToolCalled', name: 'Bash', args: {} }),
      step(2, { type: 'ResponseCanceled' }),
      step(3, { type: 'CodingAgentIdled' }),
    ]);
    expect(exchangeStatus(exchange, '', true)).toBe('canceled');
  });

  it('returns "aborted" when ResponseAborted is present (system-initiated)', () => {
    const exchange = makeExchange(msg(), [
      step(1, { type: 'TextStreamed', text: 'partial' }),
      step(2, { type: 'ResponseAborted' }),
    ]);
    expect(exchangeStatus(exchange, '', true)).toBe('aborted');
    expect(exchangeStatus(exchange, '', false)).toBe('aborted');
  });

  it('ResponseAborted after CodingAgentIdled = done (abort is from system prompt crash)', () => {
    // When CC completes work (CodingAgentIdled) and then a system-injected prompt
    // (e.g., auto-harden) crashes (ResponseAborted), the user's work was done.
    // The abort was from a system prompt, not the user's request.
    const exchange = makeExchange(msg(), [
      step(1, { type: 'SessionStarted', session_id: 's1' }),
      step(2, { type: 'CodingAgentIdled' }),
      step(3, { type: 'ResponseAborted' }),
    ]);
    expect(exchangeStatus(exchange, '', true)).toBe('done');
  });

  it('ResponseAborted before CodingAgentIdled = aborted (genuine crash)', () => {
    // When CC crashes mid-work (ResponseAborted) and then somehow goes idle
    // (e.g., recovery), the abort was real — the user's work was interrupted.
    const exchange = makeExchange(msg(), [
      step(1, { type: 'SessionStarted', session_id: 's1' }),
      step(2, { type: 'CodingAgentToolCalled', name: 'Edit', args: {} }),
      step(3, { type: 'ResponseAborted' }),
      step(4, { type: 'CodingAgentIdled' }),
    ]);
    expect(exchangeStatus(exchange, '', true)).toBe('aborted');
  });

  it('new SessionStarted after SessionEnded resets aborted check (hardening handoff)', () => {
    // Bug: when CC finishes and a hardening session starts in the same exchange,
    // SessionEnded from the first session + CC work events (which reset isComplete
    // and isCCWaiting) triggered the aborted check: isSessionEnded && !isComplete && !isCCWaiting.
    // SessionStarted must reset isSessionEnded so the hardening session isn't aborted.
    const exchange = makeExchange(msg(), [
      step(1, { type: 'SessionStarted', session_id: 's1' }),
      step(2, { type: 'CodingAgentToolCalled', name: 'Edit', args: {} }),
      step(3, { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok' }),
      step(4, { type: 'ResponseGenerated' }),
      step(5, { type: 'CodingAgentIdled' }),
      step(6, { type: 'SessionEnded', reason: 'changes_proposed' }),
      // Hardening session starts
      step(7, { type: 'SessionStarted', session_id: 's2' }),
      step(8, { type: 'CodingAgentToolCalled', name: 'Read', args: {} }),
      step(9, { type: 'CodingAgentToolResult', name: 'Read', result: 'ok' }),
    ]);
    // Should be 'cc-working' (hardening in progress), NOT 'aborted'
    expect(exchangeStatus(exchange, '', true, false, true)).toBe('cc-working');
  });

  it('SessionEnded(changes_applied) after CodingAgentPromptSent does not flash aborted', () => {
    // Bug: after apply, hardening follow-ups (CodingAgentPromptSent) reset isCCWaiting.
    // A subsequent SessionEnded(changes_applied) + ChangeApplied would briefly produce
    // 'aborted' via `isSessionEnded && !isComplete && !isCCWaiting` before the next
    // CodingAgentIdled arrived. Fix: deliberate lifecycle reasons don't set isSessionEnded.
    const exchange = makeExchange(msg(), [
      step(1, { type: 'SessionStarted', session_id: 's1' }),
      step(2, { type: 'CodingAgentToolCalled', name: 'Edit', args: {} }),
      step(3, { type: 'CodingAgentIdled', has_changes: true }),
      // apply_now sends hardening follow-up → resets isCCWaiting
      step(4, { type: 'CodingAgentPromptSent', text: 'harden' }),
      step(5, { type: 'CodingAgentToolCalled', name: 'Bash', args: {} }),
      step(6, { type: 'CodingAgentIdled', has_changes: true }),
      // Tier 2 apply: SessionEnded + ChangeApplied
      step(7, { type: 'SessionEnded', reason: 'changes_applied' } as StoredEvent),
      step(8, { type: 'ChangeApplied', change_id: 'c1' }),
    ]);
    // Must be 'done', not 'aborted'
    expect(exchangeStatus(exchange, '', true, false, true)).toBe('done');
  });

  it('SessionEnded(changes_applied) without prior CodingAgentIdled is terminal (done)', () => {
    // Even if CodingAgentIdled was never emitted (auto-harden bail-out, etc.),
    // a deliberate SessionEnded with a normal lifecycle reason marks the CC
    // exchange as terminal — must NOT show 'aborted' nor stay on 'cc-working'.
    const exchange = makeExchange(msg(), [
      step(1, { type: 'SessionStarted', session_id: 's1' }),
      step(2, { type: 'CodingAgentToolCalled', name: 'Edit', args: {} }),
      step(3, { type: 'SessionEnded', reason: 'changes_applied' } as StoredEvent),
    ]);
    expect(exchangeStatus(exchange, '', true, false, true)).toBe('done');
  });

  it('failed takes priority over aborted', () => {
    const exchange = makeExchange(msg(), [
      step(1, { type: 'ResponseAborted' }),
      step(2, { type: 'ResponseFailed', error: 'boom' }),
    ]);
    expect(exchangeStatus(exchange, '', true)).toBe('error');
  });

  // ── Engine restart / shutdown scenarios ──
  // During CC shutdown, the engine interrupts the CC process. CC produces a
  // Result event, then goes idle. No SessionEnded is emitted during shutdown
  // (the code bails out early to preserve the worktree for recovery).

  it('CC shutdown: ResponseAborted + CodingAgentIdled shows "aborted" (correct path)', () => {
    // After fix: backend emits ResponseAborted during shutdown (system-initiated).
    // CodingAgentIdled follows (CC went idle after interrupt). No SessionEnded.
    const exchange = makeExchange(msg(), [
      step(1, { type: 'SessionStarted', session_id: 's1' }),
      step(2, { type: 'CodingAgentToolCalled', name: 'Edit', args: {} }),
      step(3, { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok' }),
      step(4, { type: 'ResponseAborted', text: 'partial work' }),
      step(5, { type: 'CodingAgentIdled', has_changes: false }),
    ]);
    // ResponseAborted takes priority over CodingAgentIdled
    expect(exchangeStatus(exchange, '', true, false, true)).toBe('aborted');
  });

  it('CC shutdown: ResponseCanceled + CodingAgentIdled shows "canceled" (the bug this fixes)', () => {
    // Bug: backend emitted ResponseCanceled during shutdown (user_hit_stop was
    // set by interrupt handler, but shutting_down flag was not checked in the
    // Result handler). Fix is in agent_session — check shutting_down first.
    // Without the fix, this exchange shows "Canceled" instead of "Aborted".
    const exchange = makeExchange(msg(), [
      step(1, { type: 'SessionStarted', session_id: 's1' }),
      step(2, { type: 'CodingAgentToolCalled', name: 'Edit', args: {} }),
      step(3, { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok' }),
      step(4, { type: 'ResponseCanceled', text: 'partial work' }),
      step(5, { type: 'CodingAgentIdled', has_changes: false }),
    ]);
    // Frontend correctly derives status from the event type — "canceled" is
    // technically correct given the events. The fix is to emit the right event.
    expect(exchangeStatus(exchange, '', true, false, true)).toBe('canceled');
  });
});
