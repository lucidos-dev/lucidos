/**
 * Regression: a just-answered question / permission divider must never flash
 * "Aborted" during the answer→resume gap.
 *
 * Live repro (workspace "dev", thread c677a357, "Investigate Worktree Cleanup
 * Triggers"): the user answered an `ask_user_question` (Selected "Hold off"),
 * the loop resumed and produced a clean `ResponseGenerated` — yet the divider
 * briefly/persistently read "Aborted".
 *
 * Root cause: the stale-`'aborted'` detector in `exchangeStatus`
 * (`threadIdle && isLast && !isComplete && hasSteps`) fired for the answered
 * divider whenever the client `meta.status` still read `waiting_for_user_answer`
 * — `isThreadQuiescent('waiting_for_user_answer') === true`, so `threadIdle` was
 * true even though the thread was resuming, not crashed. The backend sets
 * `running` on `UserQuestionAnswered` / `CodingAgentPermissionResolved` /
 * `CommandPermissionResolved`, but that aggregate can lag the client (reload
 * mid-resume, an observing device, the answer→resume gap), so the stale-detector
 * must not key off `threadIdle` alone for a thread the backend reports as
 * `waiting_for_user_answer`.
 *
 * Fix: `exchangeStatus` takes `threadAwaitingAnswer` (status ===
 * 'waiting_for_user_answer') and skips the stale-`'aborted'` detector when set —
 * a parked / resuming thread is never crashed. A genuine crash settles to
 * `idle`/`failed`, so the detector still fires there.
 */
import { describe, it, expect } from 'vitest';
import {
  exchangeStatus,
  groupIntoExchanges,
  handleEvent,
  makeOptimisticThreadState,
  type Exchange,
  type StoredEvent,
  type ThreadState,
} from '../thread-events';

const TS = '2026-06-16T17:36:27Z';

/** A question divider that the user just answered, with no resume terminal yet. */
function answeredQuestionDivider(): Exchange {
  return {
    userEvent: {
      type: 'UserQuestionAsked',
      tool_use_id: 'tu-1',
      cc_session_id: '',
      question: 'Proceed?',
      options: [{ id: 'opt-2', label: 'Hold off' }],
    } as StoredEvent,
    userSeq: 3,
    steps: [
      { seq: 4, event: { type: 'UserQuestionAnswered', tool_use_id: 'tu-1', answer: { kind: 'Selected', option_id: 'opt-2' } } as StoredEvent },
    ],
  };
}

describe('answered divider never flashes Aborted during resume', () => {
  // The bug: stale client status `waiting_for_user_answer` ⇒ threadIdle=true,
  // but the thread is resuming, so the divider must NOT read 'aborted'.
  it('chat question divider with stale waiting status reads as resuming, not aborted', () => {
    const divider = answeredQuestionDivider();
    // threadIdle=true (status quiescent), threadAwaitingAnswer=true (status ===
    // 'waiting_for_user_answer'). Before the fix this returned 'aborted'.
    const status = exchangeStatus(divider, '', /*isLast*/ true, /*hasPriorActive*/ false, /*threadIsCC*/ false, /*threadIdle*/ true, /*threadAwaitingAnswer*/ true);
    expect(status).not.toBe('aborted');
  });

  it('CC question divider with stale waiting status reads coding-agent-working, not aborted', () => {
    const divider = answeredQuestionDivider();
    const status = exchangeStatus(divider, '', true, false, /*threadIsCC*/ true, /*threadIdle*/ true, /*threadAwaitingAnswer*/ true);
    expect(status).not.toBe('aborted');
    expect(status).toBe('coding-agent-working');
  });

  it('resolved permission divider with stale waiting status is not aborted', () => {
    const divider: Exchange = {
      userEvent: { type: 'CommandPermissionRequested', request_id: 'creq-1', tool_use_id: 'tu-1', tool_name: 'run_python', command: 'git pull', summary: 'pull' } as StoredEvent,
      userSeq: 2,
      steps: [
        { seq: 3, event: { type: 'CommandPermissionResolved', request_id: 'creq-1', allowed: true } as StoredEvent },
      ],
    };
    expect(exchangeStatus(divider, '', true, false, false, /*threadIdle*/ true, /*threadAwaitingAnswer*/ true)).not.toBe('aborted');
  });

  // The distinguisher must remain intact: a genuine crash settles to idle, NOT
  // waiting_for_user_answer, so the stale-detector still fires there.
  it('genuinely idle answered divider (crash, status NOT waiting) still reads aborted', () => {
    const divider = answeredQuestionDivider();
    // threadIdle=true, threadAwaitingAnswer=false (status idle/failed = crash).
    const status = exchangeStatus(divider, '', true, false, false, /*threadIdle*/ true, /*threadAwaitingAnswer*/ false);
    expect(status).toBe('aborted');
  });

  // Full live sequence through the real grouping pipeline: the answered divider
  // resumes and completes — must end 'done' and never 'aborted' at any prefix.
  it('full chat ask_user_question answer→resume sequence ends done, never aborted', () => {
    const map = new Map<string, ThreadState>();
    const id = 'thread-1';
    map.set(id, makeOptimisticThreadState({ id, title: 'x', channel: 'chat', initiator: 'user', eventsLoaded: true, timestamp: TS, status: 'running' }));

    // `request_event_id` / `channel` / `success` are added to the wire payload by
    // Rust's EventMeta at runtime but are not on the legacy-tolerant TS union, so
    // each literal is cast `as StoredEvent` (the established test pattern).
    const seqEvents: Array<[number, StoredEvent, string]> = [
      [1, { type: 'MessageReceived', text: 'investigate', _eventId: 'mr-1' } as StoredEvent, TS],
      [2, { type: 'ToolCalled', name: 'ask_user_question', args: {}, _eventId: 'tc-1', request_event_id: 'mr-1' } as StoredEvent, TS],
      [3, { type: 'UserQuestionAsked', tool_use_id: 'tu-1', cc_session_id: '', question: 'Proceed?', options: [{ id: 'opt-2', label: 'Hold off' }], channel: 'chat' } as StoredEvent, TS],
      [4, { type: 'UserQuestionAnswered', tool_use_id: 'tu-1', answer: { kind: 'Selected', option_id: 'opt-2' } } as StoredEvent, TS],
      [5, { type: 'ToolResult', name: 'ask_user_question', result: '{"Proceed?":"Hold off"}', success: true, tool_called_event_id: 'tc-1', request_event_id: 'mr-1' } as StoredEvent, TS],
      [6, { type: 'ThoughtStreamed', text: 'Context: 33931 tokens', request_event_id: 'mr-1' } as StoredEvent, TS],
      [7, { type: 'TextStreamed', text: 'OK, holding off.', request_event_id: 'mr-1' } as StoredEvent, '2026-06-16T17:36:32Z'],
      [8, { type: 'ResponseGenerated', text: 'OK, holding off.', request_event_id: 'mr-1' } as StoredEvent, '2026-06-16T17:36:32Z'],
    ];

    // Worst-case client: status pinned at `waiting_for_user_answer` (the running
    // aggregate never reached us) — exactly the condition that produced the bug.
    for (const [seq, event, created] of seqEvents) {
      handleEvent(map, id, seq, event, created);
      const thread = map.get(id)!;
      const exchanges = groupIntoExchanges(thread.events);
      const last = exchanges[exchanges.length - 1];
      const status = exchangeStatus(last, '', /*isLast*/ true, false, false, /*threadIdle*/ true, /*threadAwaitingAnswer*/ true);
      expect(status, `prefix through seq ${seq}`).not.toBe('aborted');
    }

    const exchanges = groupIntoExchanges(map.get(id)!.events);
    const last = exchanges[exchanges.length - 1];
    expect(last.userEvent.type).toBe('UserQuestionAsked');
    // The resume terminal lands in the divider → done.
    expect(exchangeStatus(last, '', true, false, false, true, false)).toBe('done');
  });
});
