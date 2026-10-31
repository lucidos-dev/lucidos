/**
 * A message sent mid-turn supersedes the running coding-agent turn. The engine
 * records the cancel and the message together, so the old session's idle can
 * land after the message. That idle closes the old turn: the new turn's
 * Thinking row stays live until its own work ends.
 */
import { describe, it, expect } from 'vitest';
import {
  computeExchanges,
  exchangeSteps,
  makeOptimisticThreadState,
  type Exchange,
  type StoredEvent,
} from '../thread-events';

function ev(seq: number, e: object, id?: string): readonly [number, StoredEvent] {
  const created = `2026-10-26T14:12:${String(seq).padStart(2, '0')}Z`;
  return [seq, { ...e, created, ...(id ? { _eventId: id } : {}) } as StoredEvent] as const;
}

const session = { channel: 'claude_code', request_event_id: 'm1' };
const message = (text: string) => ({ type: 'MessageReceived', text, channel: 'claude_code' });
const read = (id: string) => ({ type: 'CodingAgentInputRead', input_event_id: id, ...session });
const idled = { type: 'CodingAgentIdled', has_changes: true, ...session };

const superseded = [
  ev(1, message('build it'), 'm1'),
  ev(2, read('m1')),
  ev(3, { type: 'CodingAgentTextStreamed', text: 'building', ...session }),
  ev(4, { type: 'ResponseCanceled', cause: 'superseded_by_followup', ...session }),
  ev(5, message('hand off'), 'm2'),
];
const takenOver = [
  ev(7, { type: 'CodingAgentPromptSent', text: 'hand off', input_event_id: 'm2', ...session }),
  ev(8, read('m2')),
];

function fold(events: (readonly [number, StoredEvent])[], live: boolean): Exchange[] {
  const thread = makeOptimisticThreadState({
    id: 't1',
    title: 'A coding-agent thread',
    channel: 'claude_code',
    initiator: 'user',
    eventsLoaded: true,
  });
  for (const [seq, event] of events) {
    thread.events.set(seq, event);
    if (live) computeExchanges(thread);
  }
  return computeExchanges(thread);
}

const byText = (exchanges: Exchange[], text: string) =>
  exchanges.find(ex => ex.userEvent.type === 'MessageReceived' && ex.userEvent.text === text)!;
const stepTypes = (ex: Exchange) => ex.steps.map(s => s.event.type);

describe.each([
  ['in one load', false],
  ['as each event arrives', true],
])('a superseded turn whose idle lands after the next message, %s', (_, live) => {
  const exchanges = fold([...superseded, ev(6, idled), ...takenOver], live);

  it('closes the superseded turn', () => {
    expect(stepTypes(byText(exchanges, 'build it'))).toContain('CodingAgentIdled');
  });

  it('keeps the new turn thinking', () => {
    const next = byText(exchanges, 'hand off');
    expect(stepTypes(next)).not.toContain('CodingAgentIdled');
    expect(exchangeSteps(next, true).map(s => [s.description, s.outcome]))
      .toEqual([['Thinking', 'pending']]);
  });
});

// An unfinished turn proposes nothing, and the engine records that before the
// idle. The withheld proposal is the superseded turn's, not the new message's.
describe.each([
  ['in one load', false],
  ['as each event arrives', true],
])('a superseded turn that withholds its proposal before the idle, %s', (_, live) => {
  const withheld = ev(6, { type: 'ProposalWithheld', reason: 'turn_incomplete', files: ['src/a.rs'], branch_name: 'b', channel: 'claude_code' });
  const exchanges = fold([...superseded, withheld, ev(6.5, idled), ...takenOver], live);

  it('closes the superseded turn', () => {
    expect(stepTypes(byText(exchanges, 'build it'))).toEqual(expect.arrayContaining(['ProposalWithheld', 'CodingAgentIdled']));
  });

  it('keeps the new turn thinking', () => {
    const next = byText(exchanges, 'hand off');
    expect(stepTypes(next)).not.toContain('CodingAgentIdled');
    expect(exchangeSteps(next, true).map(s => [s.description, s.outcome]))
      .toEqual([['Thinking', 'pending']]);
  });
});

it('closes the superseded turn past a message queued behind it', () => {
  const [sent, readIt, said, canceled, handOff] = superseded;
  const exchanges = fold([
    sent, readIt, said,
    [30, { ...message('also check the totals'), created: '2026-10-26T14:12:03.500Z', _eventId: 'm-queued' } as StoredEvent],
    canceled, handOff,
    ev(6, idled),
    ...takenOver,
  ], false);
  expect(stepTypes(byText(exchanges, 'build it'))).toContain('CodingAgentIdled');
  expect(stepTypes(byText(exchanges, 'hand off'))).not.toContain('CodingAgentIdled');
});

it('lets the new turn end on its own idle', () => {
  const exchanges = fold([
    ...superseded,
    ev(6, idled),
    ...takenOver,
    ev(9, { type: 'CodingAgentTextStreamed', text: 'handing off', ...session }),
    ev(10, { type: 'ResponseGenerated', text: 'handing off', ...session }),
    ev(11, idled),
  ], false);
  expect(stepTypes(byText(exchanges, 'build it')).filter(t => t === 'CodingAgentIdled')).toHaveLength(1);
  expect(stepTypes(byText(exchanges, 'hand off'))).toContain('CodingAgentIdled');
});
