// Equivalence suite for the incremental exchange-grouping cache.
//
// `computeExchanges` memoizes a resumable fold per thread (keyed on the
// `thread.events` Map object) so a streaming token doesn't re-sort and
// re-walk the whole event history every frame. The contract: its result is
// ALWAYS deep-equal to a from-scratch `groupIntoExchanges` over the same
// events. These tests replay realistic event sequences step-by-step and
// assert that equivalence after EVERY event — including the sequences that
// must force a full rebuild (out-of-order arrival, legacy rerun-in-place
// aborts, missing `created` timestamps).

import { describe, it, expect } from 'vitest';
import { makeThreadState } from './thread-events-helpers';
import {
  computeExchanges,
  groupIntoExchanges,
  handleEvent,
  type ThreadEvent,
  type ThreadState,
} from '../thread-events';

/** Strictly-increasing ISO timestamps, n seconds past a fixed base. */
const at = (n: number) =>
  new Date(Date.UTC(2026, 5, 1, 0, 0, 0, 0) + n * 1000).toISOString();

/** Assert the cached/incremental result equals a from-scratch full pass.
 *  The reference runs on a CLONE of the events map so it cannot share any
 *  cache entry with the incremental path. `revision` is cache bookkeeping
 *  (a mutation counter for memoized renderers), not grouping semantics —
 *  strip it before the deep compare. */
function expectMatchesFull(thread: ThreadState): void {
  const incremental = computeExchanges(thread).map(({ revision: _r, ...rest }) => rest);
  const reference = groupIntoExchanges(new Map(thread.events)).map(
    ({ revision: _r, ...rest }) => rest,
  );
  expect(incremental).toEqual(reference);
}

/** Replay events through handleEvent with increasing timestamps, asserting
 *  incremental ≡ full after every single step. */
function replay(
  events: Array<{ seq: number; event: ThreadEvent }>,
  channel: 'chat' | 'claude_code' = 'chat',
): ThreadState {
  const thread = makeThreadState();
  thread.meta.channel = channel;
  const map = new Map([['thread-1', thread]]);
  events.forEach(({ seq, event }, i) => {
    handleEvent(map, 'thread-1', seq, event, at(i + 1), `evt-${seq}`);
    expectMatchesFull(thread);
  });
  return thread;
}

describe('incremental grouping ≡ full grouping', () => {
  it('basic chat turn with streaming tokens and a tool call', () => {
    replay([
      { seq: 1, event: { type: 'MessageReceived', text: 'hi' } as ThreadEvent },
      { seq: 2, event: { type: 'TextStreamed', text: 'thinking ' } as ThreadEvent },
      { seq: 3, event: { type: 'ToolCalled', name: 'load_knowhow', args: {} } as ThreadEvent },
      { seq: 4, event: { type: 'ToolResult', name: 'load_knowhow', result: 'ok' } as ThreadEvent },
      { seq: 5, event: { type: 'TextStreamed', text: 'answer' } as ThreadEvent },
      { seq: 6, event: { type: 'ResponseGenerated', text: 'answer' } as ThreadEvent },
    ]);
  });

  it('multi-turn chat with follow-ups', () => {
    replay([
      { seq: 1, event: { type: 'MessageReceived', text: 'first' } as ThreadEvent },
      { seq: 2, event: { type: 'TextStreamed', text: 'r1' } as ThreadEvent },
      { seq: 3, event: { type: 'ResponseGenerated', text: 'r1' } as ThreadEvent },
      { seq: 4, event: { type: 'MessageReceived', text: 'second' } as ThreadEvent },
      { seq: 5, event: { type: 'TextStreamed', text: 'r2' } as ThreadEvent },
      { seq: 6, event: { type: 'ResponseGenerated', text: 'r2' } as ThreadEvent },
      { seq: 7, event: { type: 'MessageReceived', text: 'third' } as ThreadEvent },
      { seq: 8, event: { type: 'ResponseGenerated', text: 'r3' } as ThreadEvent },
    ]);
  });

  it('CC turn with permission divider and tool-result re-routing', () => {
    replay(
      [
        { seq: 1, event: { type: 'MessageReceived', text: 'fix it' } as ThreadEvent },
        { seq: 2, event: { type: 'CodingAgentTextStreamed', text: 'on it', coding_agent: 'claude-code' } as ThreadEvent },
        { seq: 3, event: { type: 'CodingAgentToolCalled', name: 'Bash', args: {}, tool_use_id: 'tu-1', coding_agent: 'claude-code' } as ThreadEvent },
        { seq: 4, event: { type: 'CodingAgentPermissionRequest', request_id: 'pr-1', tool_use_id: 'tu-perm', tool_name: 'Bash', input: {}, summary: 'run a command' } as ThreadEvent },
        { seq: 5, event: { type: 'CodingAgentPermissionResolved', request_id: 'pr-1', allowed: true, coding_agent: 'claude-code' } as ThreadEvent },
        // Result for the seq-3 call lands AFTER the divider — must re-route
        // to the call's exchange in both paths.
        { seq: 6, event: { type: 'CodingAgentToolResult', name: 'Bash', result: 'ok', tool_use_id: 'tu-1', coding_agent: 'claude-code' } as ThreadEvent },
        { seq: 7, event: { type: 'CodingAgentTextStreamed', text: 'done', coding_agent: 'claude-code' } as ThreadEvent },
        { seq: 8, event: { type: 'CodingAgentIdled', has_changes: false, coding_agent: 'claude-code' } as ThreadEvent },
      ],
      'claude_code',
    );
  });

  it('question divider: overtaken flag and answer re-routing', () => {
    replay([
      { seq: 1, event: { type: 'MessageReceived', text: 'ask me' } as ThreadEvent },
      { seq: 2, event: { type: 'ToolCalled', name: 'ask_user_question', args: {} } as ThreadEvent },
      { seq: 3, event: { type: 'UserQuestionAsked', tool_use_id: 'q-1', cc_session_id: 's-1', question: 'pick one', options: [] } as ThreadEvent },
      // Agent races past the unanswered question — flips questionOvertaken.
      { seq: 4, event: { type: 'TextStreamed', text: 'continuing anyway' } as ThreadEvent },
      // The answer routes back to the divider and clears the flag.
      { seq: 5, event: { type: 'UserQuestionAnswered', tool_use_id: 'q-1', answer: { kind: 'Selected', option_id: 'a' } } as ThreadEvent },
      { seq: 6, event: { type: 'ResponseGenerated', text: 'done' } as ThreadEvent },
    ]);
  });

  it('abort opens a boundary exchange', () => {
    replay([
      { seq: 1, event: { type: 'MessageReceived', text: 'go' } as ThreadEvent },
      { seq: 2, event: { type: 'TextStreamed', text: 'partial' } as ThreadEvent },
      { seq: 3, event: { type: 'ResponseAborted', cause: 'safety_net' } as ThreadEvent },
      { seq: 4, event: { type: 'MessageReceived', text: 'again' } as ThreadEvent },
      { seq: 5, event: { type: 'ResponseGenerated', text: 'ok' } as ThreadEvent },
    ]);
  });

  it('legacy rerun-in-place: terminal AFTER abort forces a coherent rebuild', () => {
    // ResponseAborted shares request_event_id with a LATER ResponseGenerated —
    // the full pass retro-classifies the abort as a superseded step, so the
    // incremental path must detect the late terminal and rebuild.
    replay([
      { seq: 1, event: { type: 'MessageReceived', text: 'go' } as ThreadEvent },
      { seq: 2, event: { type: 'ResponseAborted', cause: 'engine_shutdown', request_event_id: 'req-1' } as unknown as ThreadEvent },
      { seq: 3, event: { type: 'TextStreamed', text: 'rerun output', request_event_id: 'req-1' } as unknown as ThreadEvent },
      { seq: 4, event: { type: 'ResponseGenerated', text: 'rerun output', request_event_id: 'req-1' } as unknown as ThreadEvent },
      { seq: 5, event: { type: 'MessageReceived', text: 'next' } as ThreadEvent },
    ]);
  });

  it('abort AFTER matching terminal is superseded at fold time', () => {
    replay([
      { seq: 1, event: { type: 'MessageReceived', text: 'go' } as ThreadEvent },
      { seq: 2, event: { type: 'ResponseGenerated', text: 'ok', request_event_id: 'req-9' } as unknown as ThreadEvent },
      { seq: 3, event: { type: 'ResponseAborted', cause: 'unknown', request_event_id: 'req-9' } as unknown as ThreadEvent },
      { seq: 4, event: { type: 'MessageReceived', text: 'next' } as ThreadEvent },
    ]);
  });

  it('out-of-order created timestamp forces a rebuild and stays equivalent', () => {
    const thread = makeThreadState();
    const map = new Map([['thread-1', thread]]);
    handleEvent(map, 'thread-1', 1, { type: 'MessageReceived', text: 'a' } as ThreadEvent, at(10), 'evt-1');
    expectMatchesFull(thread);
    handleEvent(map, 'thread-1', 2, { type: 'ResponseGenerated', text: 'r' } as ThreadEvent, at(20), 'evt-2');
    expectMatchesFull(thread);
    // A refresh replay delivers a missed event whose created is EARLIER than
    // the last folded event — sorted position is in the middle, not the end.
    handleEvent(map, 'thread-1', 3, { type: 'MessageReceived', text: 'missed' } as ThreadEvent, at(15), 'evt-3');
    expectMatchesFull(thread);
    handleEvent(map, 'thread-1', 4, { type: 'MessageReceived', text: 'after' } as ThreadEvent, at(30), 'evt-4');
    expectMatchesFull(thread);
  });

  it('events without created timestamps stay equivalent (legacy rows)', () => {
    const thread = makeThreadState();
    const map = new Map([['thread-1', thread]]);
    handleEvent(map, 'thread-1', 1, { type: 'MessageReceived', text: 'old row' } as ThreadEvent, undefined as unknown as string, 'evt-1');
    expectMatchesFull(thread);
    handleEvent(map, 'thread-1', 2, { type: 'TextStreamed', text: 'x' } as ThreadEvent, undefined as unknown as string, 'evt-2');
    expectMatchesFull(thread);
    handleEvent(map, 'thread-1', 3, { type: 'ResponseGenerated', text: 'x' } as ThreadEvent, at(3), 'evt-3');
    expectMatchesFull(thread);
  });

  it('keeps exchange object identity stable across appends (memo-friendly)', () => {
    const thread = replay([
      { seq: 1, event: { type: 'MessageReceived', text: 'hi' } as ThreadEvent },
      { seq: 2, event: { type: 'TextStreamed', text: 'a' } as ThreadEvent },
    ]);
    const before = computeExchanges(thread);
    const map = new Map([['thread-1', thread]]);
    handleEvent(map, 'thread-1', 3, { type: 'TextStreamed', text: 'b' } as ThreadEvent, at(99), 'evt-3');
    const after = computeExchanges(thread);
    expectMatchesFull(thread);
    // Fresh array identity (so signal subscribers fire), stable element
    // identity (so per-exchange memoization holds).
    expect(after).not.toBe(before);
    expect(after[0]).toBe(before[0]);
  });

  it('returns equal content on repeated calls with no new events', () => {
    const thread = replay([
      { seq: 1, event: { type: 'MessageReceived', text: 'hi' } as ThreadEvent },
      { seq: 2, event: { type: 'ResponseGenerated', text: 'r' } as ThreadEvent },
    ]);
    const a = computeExchanges(thread);
    const b = computeExchanges(thread);
    expect(a).toEqual(b);
    expect(a[0]).toBe(b[0]);
  });

  it('pending user messages bypass the cache and stay equivalent on clear', () => {
    const thread = replay([
      { seq: 1, event: { type: 'MessageReceived', text: 'hi' } as ThreadEvent },
      { seq: 2, event: { type: 'ResponseGenerated', text: 'r' } as ThreadEvent },
    ]);
    thread.pendingUserMessages.push({
      eventId: 'pending-1',
      text: 'queued follow-up',
      created: at(50),
    });
    // Synthetic MessageReceived renders as a new trailing exchange.
    const withPending = computeExchanges(thread);
    expect(withPending[withPending.length - 1].userEvent.type).toBe('MessageReceived');
    expect((withPending[withPending.length - 1].userEvent as { text?: string }).text).toBe('queued follow-up');
    // The real MessageReceived arrives (same eventId) — pending is consumed
    // by handleEvent and the cached path resumes, still equivalent.
    const map = new Map([['thread-1', thread]]);
    handleEvent(map, 'thread-1', 3, { type: 'MessageReceived', text: 'queued follow-up' } as ThreadEvent, at(51), 'pending-1');
    expect(thread.pendingUserMessages).toHaveLength(0);
    expectMatchesFull(thread);
  });

  it('abort and matching terminal arriving in ONE batch stay equivalent', () => {
    // Regression: the validation pass used to check appended terminals only
    // against aborts folded in PREVIOUS calls — an abort earlier in the SAME
    // batch was invisible, so the incremental path kept a boundary exchange
    // the from-scratch pass marks legacy-superseded. Batch = multiple
    // handleEvent calls between two computeExchanges evaluations (an SSE
    // flush or refresh replay on an unfocused-but-cached thread).
    const thread = makeThreadState();
    const map = new Map([['thread-1', thread]]);
    handleEvent(map, 'thread-1', 1, { type: 'MessageReceived', text: 'go' } as ThreadEvent, at(1), 'evt-1');
    expectMatchesFull(thread); // warm the cache
    handleEvent(map, 'thread-1', 2, { type: 'ResponseAborted', cause: 'engine_shutdown', request_event_id: 'req-b' } as unknown as ThreadEvent, at(2), 'evt-2');
    handleEvent(map, 'thread-1', 3, { type: 'ResponseGenerated', text: 'rerun', request_event_id: 'req-b' } as unknown as ThreadEvent, at(3), 'evt-3');
    // No computeExchanges between the two — both land in one appended batch.
    expectMatchesFull(thread);
  });

  it('bumps revision on touched exchanges so memoized renderers repaint', () => {
    const thread = replay([
      { seq: 1, event: { type: 'MessageReceived', text: 'hi' } as ThreadEvent },
      { seq: 2, event: { type: 'TextStreamed', text: 'a' } as ThreadEvent },
    ]);
    const before = computeExchanges(thread)[0];
    const beforeRevision = before.revision ?? 0;
    const map = new Map([['thread-1', thread]]);
    handleEvent(map, 'thread-1', 3, { type: 'TextStreamed', text: 'b' } as ThreadEvent, at(99), 'evt-3');
    const after = computeExchanges(thread)[0];
    // Same object (identity-stable), but the captured revision moved — this
    // is what lets chatExchangePropsEqual detect the in-place steps.push.
    expect(after).toBe(before);
    expect(after.revision ?? 0).toBeGreaterThan(beforeRevision);
  });

  it('a long streaming turn stays equivalent at every token', () => {
    const events: Array<{ seq: number; event: ThreadEvent }> = [
      { seq: 1, event: { type: 'MessageReceived', text: 'stream a lot' } as ThreadEvent },
    ];
    for (let i = 0; i < 120; i++) {
      events.push({ seq: i + 2, event: { type: 'TextStreamed', text: `tok${i} ` } as ThreadEvent });
    }
    events.push({ seq: 200, event: { type: 'ResponseGenerated', text: 'done' } as ThreadEvent });
    replay(events);
  });
});
