/** "Aborted" is a claim that something aborted the turn, so an abort event must
 *  back it. The engine records every real abort as one: a crash, a kill, a
 *  shutdown, a stale settle, a recovery after restart.
 *
 *  The client's copy of the thread status lags the events and can be clobbered
 *  by a snapshot read. So "the thread reads idle and this exchange has no
 *  terminator" is not evidence of an abort: the turn may be about to read
 *  "Working".
 *
 *  So the tests sweep every status combination over turns with no abort in
 *  them, and assert that none reads "Aborted".
 */
import { describe, it, expect } from 'vitest';
import { ev } from './call-fixtures';
import type { Recorded } from './call-fixtures';
import {
  exchangeStatus,
  exchangeVerdict,
  groupIntoExchanges,
  type Exchange,
  type ExchangeVerdict,
  type StoredEvent,
} from '../thread-events';

const MSG = 'msg-1';
const Q = 'toolu_q1';

function log(...events: Recorded[]): Map<number, StoredEvent> {
  return new Map(events.map((e, i) => ev(i + 1, e)));
}

/** Turns that no abort touched, each cut off at the point a lagging status
 *  can catch it: steps landed, no terminator yet. */
const NO_ABORT_SHAPES: Record<string, Map<number, StoredEvent>> = {
  'a chat turn mid-flight': log(
    { type: 'MessageReceived', text: 'do the thing', mode: 'human', _eventId: MSG },
    { type: 'ToolCalled', name: 'list_files', args: {}, request_event_id: MSG },
  ),
  'a coding-agent turn mid-flight': log(
    { type: 'MessageReceived', text: 'fix bug', channel: 'claude_code' },
    { type: 'SessionStarted', session_id: 's1' },
    { type: 'CodingAgentToolCalled', name: 'Read', args: {} },
  ),
  'a coding-agent follow-up after an idle': log(
    { type: 'MessageReceived', text: 'fix bug', channel: 'claude_code' },
    { type: 'SessionStarted', session_id: 's1' },
    { type: 'CodingAgentIdled', has_changes: false },
    { type: 'MessageReceived', text: 'also the tests', channel: 'claude_code' },
    { type: 'CodingAgentPromptSent', text: 'also the tests' },
    { type: 'CodingAgentToolCalled', name: 'Edit', args: {} },
  ),
  'a coding-agent question just answered': log(
    { type: 'MessageReceived', text: 'fix bug', channel: 'claude_code' },
    { type: 'SessionStarted', session_id: 's1' },
    { type: 'CodingAgentToolCalled', name: 'Read', args: {} },
    { type: 'UserQuestionAsked', tool_use_id: Q, questions: [] } as unknown as Recorded,
    { type: 'UserQuestionAnswered', tool_use_id: Q, answers: [] } as unknown as Recorded,
    { type: 'CodingAgentPromptSent', text: 'answer' },
  ),
  'a coding-agent question answered after the session idled': log(
    { type: 'MessageReceived', text: 'fix bug', channel: 'claude_code' },
    { type: 'SessionStarted', session_id: 's1' },
    { type: 'UserQuestionAsked', tool_use_id: Q, questions: [] } as unknown as Recorded,
    { type: 'UserQuestionAnswered', tool_use_id: Q, answers: [] } as unknown as Recorded,
    { type: 'CodingAgentPromptSent', text: 'answer' },
    { type: 'ContinuationRequested', reason: 'answered_after_idle' } as unknown as Recorded,
    { type: 'SessionStarted', session_id: 's2' },
    { type: 'CodingAgentToolCalled', name: 'Read', args: {} },
  ),
  'a coding-agent question overtaken by work': log(
    { type: 'MessageReceived', text: 'fix bug', channel: 'claude_code' },
    { type: 'SessionStarted', session_id: 's1' },
    { type: 'UserQuestionAsked', tool_use_id: Q, questions: [] } as unknown as Recorded,
    { type: 'CodingAgentTextStreamed', text: 'carrying on' },
    { type: 'CodingAgentToolCalled', name: 'Read', args: {} },
  ),
  'a normal session end with no idle': log(
    { type: 'MessageReceived', text: 'fix bug', channel: 'claude_code' },
    { type: 'SessionStarted', session_id: 's1' },
    { type: 'CodingAgentToolCalled', name: 'Read', args: {} },
    { type: 'SessionEnded', reason: 'completed' } as unknown as Recorded,
  ),
};

const BOOLS = [false, true] as const;

describe('"Aborted" needs an abort event', () => {
  for (const [name, events] of Object.entries(NO_ABORT_SHAPES)) {
    it(`never reads aborted: ${name}`, () => {
      const exchanges = groupIntoExchanges(events);
      expect(exchanges.length).toBeGreaterThan(0);
      for (const exchange of exchanges) {
        for (const isLast of BOOLS) for (const hasPriorActive of BOOLS)
        for (const threadIsCC of BOOLS) for (const threadIdle of BOOLS)
        for (const threadAwaitingAnswer of BOOLS) for (const buffer of ['', 'tokens']) {
          const status = exchangeStatus(
            exchange, buffer, isLast, hasPriorActive, threadIsCC, threadIdle, threadAwaitingAnswer,
          );
          expect(
            status,
            `${exchange.userEvent.type} isLast=${isLast} idle=${threadIdle} awaiting=${threadAwaitingAnswer} cc=${threadIsCC}`,
          ).not.toBe('aborted');
        }
      }
    });
  }

  // The stale detector's real job survives: nothing runs on an idle thread,
  // so a turn with no terminator must not spin "Working" either.
  it('settles an unterminated turn on an idle thread as done', () => {
    for (const events of [NO_ABORT_SHAPES['a chat turn mid-flight'], NO_ABORT_SHAPES['a coding-agent turn mid-flight']]) {
      const exchanges = groupIntoExchanges(events);
      const last = exchanges[exchanges.length - 1];
      const isCC = last.userEvent.type === 'MessageReceived' && last.userEvent.channel === 'claude_code';
      expect(exchangeStatus(last, '', true, false, isCC, true)).toBe('done');
    }
  });

  it('keeps an unterminated turn working while the thread runs', () => {
    const exchanges = groupIntoExchanges(NO_ABORT_SHAPES['a coding-agent turn mid-flight']);
    expect(exchangeStatus(exchanges[0], '', true, false, true, false)).toBe('coding-agent-working');
  });

  it('still reads aborted when an abort event is there', () => {
    const exchanges = groupIntoExchanges(ABORT_SHAPES['a chat turn the engine killed']);
    expect(exchangeStatus(exchanges[0], '', true, false, false, true)).toBe('aborted');
  });

  it('refuses to type an aborted verdict without its evidence', () => {
    // @ts-expect-error an aborted verdict must name its abort event
    const forged: ExchangeVerdict = { status: 'aborted' };
    expect(forged.status).toBe('aborted');
  });
});

/** Turns an abort did end, one per kind of evidence. */
const ABORT_SHAPES: Record<string, Map<number, StoredEvent>> = {
  'a message the crash dropped before its first token': log(
    { type: 'MessageReceived', text: 'do the thing', mode: 'human', _eventId: MSG },
    { type: 'ResponseAborted', text: 'This response was interrupted by an engine restart.', cause: 'recovery_after_restart', request_event_id: MSG } as unknown as Recorded,
  ),
  'a chat turn the engine killed': log(
    { type: 'MessageReceived', text: 'do the thing', mode: 'human', _eventId: MSG },
    { type: 'ToolCalled', name: 'list_files', args: {}, request_event_id: MSG },
    { type: 'ResponseAborted', text: '', cause: 'process_killed', request_event_id: MSG } as unknown as Recorded,
  ),
  'a coding-agent session that panicked': log(
    { type: 'MessageReceived', text: 'fix bug', channel: 'claude_code' },
    { type: 'SessionStarted', session_id: 's1' },
    { type: 'CodingAgentToolCalled', name: 'Read', args: {} },
    { type: 'SessionEnded', reason: 'panic' } as unknown as Recorded,
  ),
  'a coding-agent session the engine shut down': log(
    { type: 'MessageReceived', text: 'fix bug', channel: 'claude_code' },
    { type: 'SessionStarted', session_id: 's1' },
    { type: 'CodingAgentToolCalled', name: 'Read', args: {} },
    { type: 'SessionEnded', reason: 'shutdown' } as unknown as Recorded,
  ),
  'an engine-down boundary with a drain': log(
    { type: 'MessageReceived', text: 'fix bug', channel: 'claude_code' },
    { type: 'SessionStarted', session_id: 's1' },
    { type: 'CodingAgentToolCalled', name: 'Read', args: {} },
    { type: 'ResponseAborted', text: '', cause: 'engine_shutdown' } as unknown as Recorded,
    { type: 'CodingAgentTextStreamed', text: '\n\n' },
  ),
};

/** Every event an exchange holds, its boundary included. */
function eventsOf(exchange: Exchange): StoredEvent[] {
  return [exchange.userEvent, ...exchange.steps.map(s => s.event)];
}

describe('an aborted verdict names an abort event inside its exchange', () => {
  for (const [name, events] of Object.entries(ABORT_SHAPES)) {
    it(`evidence is an abort event of the exchange: ${name}`, () => {
      let seenAborted = false;
      for (const exchange of groupIntoExchanges(events)) {
        for (const isLast of BOOLS) for (const threadIsCC of BOOLS)
        for (const threadIdle of BOOLS) for (const threadAwaitingAnswer of BOOLS) {
          const v = exchangeVerdict(exchange, '', isLast, false, threadIsCC, threadIdle, threadAwaitingAnswer);
          if (v.status !== 'aborted') continue;
          seenAborted = true;
          expect(eventsOf(exchange)).toContain(v.evidence.event);
          expect(['ResponseAborted', 'SessionEnded']).toContain(v.evidence.event.type);
        }
      }
      expect(seenAborted, `${name} never read aborted`).toBe(true);
    });
  }
});
