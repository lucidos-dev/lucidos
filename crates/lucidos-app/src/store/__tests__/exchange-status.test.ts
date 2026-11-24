import { describe, it, expect } from 'vitest';
import {
  exchangeStatus,
  type StoredEvent,
} from '../thread-events';
import { makeExchange, step } from './fixtures';

function msg(text = 'hi'): StoredEvent {
  return { type: 'MessageReceived', text };
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

  it('returns "canceled" for a user_stop / user_action ResponseCanceled (regression guard)', () => {
    const stop = makeExchange(msg(), [
      step(1, { type: 'CodingAgentToolCalled', name: 'Bash', args: {} }),
      step(2, { type: 'ResponseCanceled', cause: 'user_stop' }),
    ]);
    expect(exchangeStatus(stop, '', true)).toBe('canceled');
    const action = makeExchange(msg(), [
      step(1, { type: 'CodingAgentToolCalled', name: 'Bash', args: {} }),
      step(2, { type: 'ResponseCanceled', cause: 'user_action' }),
    ]);
    expect(exchangeStatus(action, '', true)).toBe('canceled');
  });

  it('renders a superseded_by_followup ResponseCanceled neutrally — "done", not "canceled"', () => {
    const exchange = makeExchange(msg(), [
      step(1, { type: 'CodingAgentToolCalled', name: 'Bash', args: {} }),
      step(2, { type: 'ResponseCanceled', cause: 'superseded_by_followup' }),
      step(3, { type: 'CodingAgentIdled' }),
    ]);
    expect(exchangeStatus(exchange, '', true)).toBe('done');
    expect(exchangeStatus(exchange, '', false)).toBe('done');
  });

  it('renders an effort_restart ResponseCanceled as work still owed', () => {
    const exchange = makeExchange(msg(), [
      step(1, { type: 'CodingAgentToolCalled', name: 'Bash', args: {} }),
      step(2, { type: 'ResponseCanceled', cause: 'effort_restart' }),
      step(3, { type: 'CodingAgentIdled' }),
    ]);
    expect(exchangeStatus(exchange, '', true)).toBe('coding-agent-working');
    expect(exchangeStatus(exchange, '', false)).toBe('done');
  });

  it('superseded_by_followup cancel is terminal "done" even before CodingAgentIdled lands', () => {
    const exchange = makeExchange(msg(), [
      step(1, { type: 'CodingAgentToolCalled', name: 'Bash', args: {} }),
      step(2, { type: 'ResponseCanceled', cause: 'superseded_by_followup' }),
    ]);
    expect(exchangeStatus(exchange, '', true)).toBe('done');
    expect(exchangeStatus(exchange, '', false)).toBe('done');
  });

  it('returns "canceled" for CC exchange canceled during startup (no SessionStarted yet)', () => {
    const exchange = makeExchange(msg(), [
      step(1, { type: 'ResponseCanceled' }),
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
    const exchange = makeExchange(msg(), [
      step(1, { type: 'SessionStarted', session_id: 's1' }),
      step(2, { type: 'CodingAgentToolCalled', name: 'bash', args: {} }),
      step(3, { type: 'SessionEnded', reason: 'shutdown' } as StoredEvent),
    ]);
    expect(exchangeStatus(exchange, '', true)).toBe('aborted');
    expect(exchangeStatus(exchange, '', false)).toBe('aborted');
  });

  it('returns "interrupted" for non-last non-CC exchange with steps (user moved on with follow-up)', () => {
    const exchange = makeExchange(msg(), [
      step(1, { type: 'TextStreamed', text: 'partial' }),
    ]);
    expect(exchangeStatus(exchange, '',  false,  false,  false,  true)).toBe('interrupted');
    expect(exchangeStatus(exchange, '',  false,  false,  false,  false)).toBe('interrupted');
  });

  it('returns "coding-agent-working" for last CC exchange without completion or idle', () => {
    const exchange = makeExchange(msg(), [
      step(1, { type: 'SessionStarted', session_id: 's1' }),
      step(2, { type: 'CodingAgentToolCalled', name: 'bash', args: {} }),
    ]);
    expect(exchangeStatus(exchange, '', true)).toBe('coding-agent-working');
  });

  it('returns "done" for a stepless ChildThreadCompleted card once the parent is quiescent or it was superseded', () => {
    const card = makeExchange(
      { type: 'ChildThreadCompleted', child_thread_id: 'c1', status: 'success', summary: 'done' } as StoredEvent,
      [],
    );
    expect(exchangeStatus(card, '',  true,  false,  true,  true)).toBe('done');
    expect(exchangeStatus(card, '',  true,  false,  false,  true)).toBe('done');
    expect(exchangeStatus(card, '',  false,  false,  false,  false)).toBe('done');
  });

  it('a stepless ChildThreadCompleted card on a RUNNING parent reads as working (pre-first-step gap)', () => {
    const card = makeExchange(
      { type: 'ChildThreadCompleted', child_thread_id: 'c1', status: 'success', summary: 'done' } as StoredEvent,
      [],
    );
    expect(exchangeStatus(card, '',  true,  false,  false,  false)).toBe('pending');
    expect(exchangeStatus(card, '',  true,  false,  true,  false)).toBe('pending');
  });

  it('a ChildThreadCompleted card WITH resume work reads coding-agent-working until idle', () => {
    const working = makeExchange(
      { type: 'ChildThreadCompleted', child_thread_id: 'c1', status: 'success', summary: 'done' } as StoredEvent,
      [step(1, { type: 'CodingAgentTextStreamed', text: 'reacting to child' })],
    );
    expect(exchangeStatus(working, '', true, false, true)).toBe('coding-agent-working');
  });

  it('returns "streaming" when streamingBuffer has content', () => {
    const exchange = makeExchange(msg());
    expect(exchangeStatus(exchange, 'partial text...', true)).toBe('streaming');
  });

  it('returns "streaming" when responseText exists but no completion event (buffer race)', () => {
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
    expect(exchangeStatus(exchange, '', true)).toBe('streaming');
  });

  it('returns "streaming" when old events exist (long-running tool call)', () => {
    const oldDate = new Date(Date.now() - 120_000).toISOString();
    const exchange = makeExchange(
      { type: 'MessageReceived', text: 'hi', created: oldDate },
      [step(1, { type: 'ToolCalled', name: 'edit_file', args: {}, created: oldDate })],
    );
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

  it('returns "done" for empty isLast chat exchange when thread is idle (no stuck "Requesting" / no false "Aborted")', () => {
    const exchange = makeExchange(msg('what was it...'));
    expect(exchangeStatus(exchange, '',  true,  false,  false,  true)).toBe('done');
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

  it('returns "done" for follow-up exchange whose content was injected into prior request', () => {
    const exchange = makeExchange(msg('men kan ikke ha dette hele tiden'), [
      step(1, {
        type: 'PromptInjected',
        text: 'men kan ikke ha dette hele tiden',
        injected_message_id: 'msg-followup',
      } as StoredEvent),
    ]);
    expect(exchangeStatus(exchange, '', true, false, false, true)).toBe('done');
    expect(exchangeStatus(exchange, '', false, false, false, true)).toBe('done');
  });

  it('ResponseAborted after CodingAgentIdled = done (abort is from system prompt crash)', () => {
    const exchange = makeExchange(msg(), [
      step(1, { type: 'SessionStarted', session_id: 's1' }),
      step(2, { type: 'CodingAgentIdled' }),
      step(3, { type: 'ResponseAborted' }),
    ]);
    expect(exchangeStatus(exchange, '', true)).toBe('done');
  });

  it('ResponseAborted before CodingAgentIdled = aborted (genuine crash)', () => {
    const exchange = makeExchange(msg(), [
      step(1, { type: 'SessionStarted', session_id: 's1' }),
      step(2, { type: 'CodingAgentToolCalled', name: 'Edit', args: {} }),
      step(3, { type: 'ResponseAborted' }),
      step(4, { type: 'CodingAgentIdled' }),
    ]);
    expect(exchangeStatus(exchange, '', true)).toBe('aborted');
  });

  it('new SessionStarted after SessionEnded resets aborted check (hardening handoff)', () => {
    const exchange = makeExchange(msg(), [
      step(1, { type: 'SessionStarted', session_id: 's1' }),
      step(2, { type: 'CodingAgentToolCalled', name: 'Edit', args: {} }),
      step(3, { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok' }),
      step(4, { type: 'ResponseGenerated' }),
      step(5, { type: 'CodingAgentIdled' }),
      step(6, { type: 'SessionEnded', reason: 'changes_proposed' }),
      step(7, { type: 'SessionStarted', session_id: 's2' }),
      step(8, { type: 'CodingAgentToolCalled', name: 'Read', args: {} }),
      step(9, { type: 'CodingAgentToolResult', name: 'Read', result: 'ok' }),
    ]);
    expect(exchangeStatus(exchange, '', true, false, true)).toBe('coding-agent-working');
  });

  it('SessionEnded(changes_applied) after CodingAgentPromptSent does not flash aborted', () => {
    const exchange = makeExchange(msg(), [
      step(1, { type: 'SessionStarted', session_id: 's1' }),
      step(2, { type: 'CodingAgentToolCalled', name: 'Edit', args: {} }),
      step(3, { type: 'CodingAgentIdled', has_changes: true }),
      step(4, { type: 'CodingAgentPromptSent', text: 'harden' }),
      step(5, { type: 'CodingAgentToolCalled', name: 'Bash', args: {} }),
      step(6, { type: 'CodingAgentIdled', has_changes: true }),
      step(7, { type: 'SessionEnded', reason: 'changes_applied' } as StoredEvent),
      step(8, { type: 'ChangeApplied', change_id: 'c1' }),
    ]);
    expect(exchangeStatus(exchange, '', true, false, true)).toBe('done');
  });

  it('SessionEnded(changes_applied) without prior CodingAgentIdled is terminal (done)', () => {
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


  it('CC shutdown: ResponseAborted + CodingAgentIdled shows "aborted" (correct path)', () => {
    const exchange = makeExchange(msg(), [
      step(1, { type: 'SessionStarted', session_id: 's1' }),
      step(2, { type: 'CodingAgentToolCalled', name: 'Edit', args: {} }),
      step(3, { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok' }),
      step(4, { type: 'ResponseAborted', text: 'partial work' }),
      step(5, { type: 'CodingAgentIdled', has_changes: false }),
    ]);
    expect(exchangeStatus(exchange, '', true, false, true)).toBe('aborted');
  });

  it('CC shutdown: ResponseCanceled + CodingAgentIdled shows "canceled" (the bug this fixes)', () => {
    const exchange = makeExchange(msg(), [
      step(1, { type: 'SessionStarted', session_id: 's1' }),
      step(2, { type: 'CodingAgentToolCalled', name: 'Edit', args: {} }),
      step(3, { type: 'CodingAgentToolResult', name: 'Edit', result: 'ok' }),
      step(4, { type: 'ResponseCanceled', text: 'partial work' }),
      step(5, { type: 'CodingAgentIdled', has_changes: false }),
    ]);
    expect(exchangeStatus(exchange, '', true, false, true)).toBe('canceled');
  });


  it('engine-restart recovery: ResponseAborted then ResponseGenerated (same request_event_id) → done', () => {
    const exchange = makeExchange(msg(), [
      step(1, { type: 'TextStreamed', text: 'partial' }),
      step(2, { type: 'ResponseAborted', request_event_id: 'req-1', text: 'restart interrupted' } as StoredEvent),
      step(3, { type: 'TextStreamed', text: 'rerun result' }),
      step(4, { type: 'ResponseGenerated', request_event_id: 'req-1', text: 'final response' } as StoredEvent),
    ]);
    expect(exchangeStatus(exchange, '', true)).toBe('done');
    expect(exchangeStatus(exchange, '', false)).toBe('done');
  });

  it('engine-restart recovery: ResponseAborted then ResponseFailed (same request_event_id) → error', () => {
    const exchange = makeExchange(msg(), [
      step(1, { type: 'TextStreamed', text: 'partial' }),
      step(2, { type: 'ResponseAborted', request_event_id: 'req-1', text: 'restart interrupted' } as StoredEvent),
      step(3, { type: 'ResponseFailed', request_event_id: 'req-1', error: 'boom on rerun' } as StoredEvent),
    ]);
    expect(exchangeStatus(exchange, '', true)).toBe('error');
    expect(exchangeStatus(exchange, '', false)).toBe('error');
  });

  it('lone ResponseAborted (no later terminal for same request_event_id) → aborted', () => {
    const exchange = makeExchange(msg(), [
      step(1, { type: 'TextStreamed', text: 'partial' }),
      step(2, { type: 'ResponseAborted', request_event_id: 'req-1', text: 'crashed' } as StoredEvent),
    ]);
    expect(exchangeStatus(exchange, '', true)).toBe('aborted');
  });

  it('two unrelated request_event_ids in one exchange aren\'t merged (sanity)', () => {
    const exchange = makeExchange(msg(), [
      step(1, { type: 'ResponseAborted', request_event_id: 'req-A', text: 'crash' } as StoredEvent),
      step(2, { type: 'ResponseGenerated', request_event_id: 'req-B', text: 'unrelated' } as StoredEvent),
    ]);
    expect(exchangeStatus(exchange, '', true)).toBe('aborted');
  });
});
