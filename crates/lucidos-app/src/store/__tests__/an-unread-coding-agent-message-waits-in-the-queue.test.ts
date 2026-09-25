/**
 * A message sent to a coding agent mid-turn waits in the queue until the agent
 * reads it. The turn that is running keeps its own steps, and the message takes
 * the steps after the read. See
 * docs/plans/2026-09-24-unread-coding-agent-messages-queue.md.
 */
import { describe, it, expect } from 'vitest';
import {
  computeExchanges,
  exchangeResponseEvents,
  exchangeSteps,
  groupIntoExchanges,
  isThinking,
  makeOptimisticThreadState,
  queuedFollowupRun,
  queuedMessagesFromExchanges,
  type Exchange,
  type StoredEvent,
  type ThreadEvent,
} from '../thread-events';

function ev(seq: number, e: object, id?: string): readonly [number, StoredEvent] {
  const created = `2026-09-24T06:13:${String(seq).padStart(2, '0')}Z`;
  return [seq, { ...e, created, ...(id ? { _eventId: id } : {}) } as StoredEvent] as const;
}

const message = (text: string) => ({ type: 'MessageReceived', text, channel: 'claude_code' });
const said = (text: string) => ({ type: 'CodingAgentTextStreamed', text, channel: 'claude_code' });
const read = (id: string): ThreadEvent => ({ type: 'CodingAgentInputRead', input_event_id: id });
const done = { type: 'ResponseGenerated', text: 'ok', channel: 'claude_code' };
const idled = { type: 'CodingAgentIdled', channel: 'claude_code' };

const texts = (ex: Exchange) =>
  ex.steps.flatMap(s => s.event.type === 'CodingAgentTextStreamed' ? [s.event.text] : []);
const byMessage = (exchanges: Exchange[], text: string) =>
  exchanges.find(ex => ex.userEvent.type === 'MessageReceived' && ex.userEvent.text === text)!;

describe('an unread coding-agent message', () => {
  const running = [
    ev(1, message('start'), 'm1'),
    ev(2, read('m1')),
    ev(3, said('working on it')),
    ev(4, message('also check the totals'), 'm2'),
    ev(5, said('still on the first job')),
  ];

  it('takes no steps before the agent reads it', () => {
    const exchanges = groupIntoExchanges(new Map(running));
    expect(texts(byMessage(exchanges, 'start'))).toEqual(['working on it', 'still on the first job']);
    expect(texts(byMessage(exchanges, 'also check the totals'))).toEqual([]);
    expect(byMessage(exchanges, 'also check the totals').awaitingRead).toBe(true);
  });

  it('takes the steps after the agent reads it', () => {
    const exchanges = groupIntoExchanges(new Map([
      ...running,
      ev(6, read('m2')),
      ev(7, said('checking the totals')),
    ]));
    expect(texts(byMessage(exchanges, 'start'))).toEqual(['working on it', 'still on the first job']);
    const second = byMessage(exchanges, 'also check the totals');
    expect(texts(second)).toEqual(['checking the totals']);
    expect(second.awaitingRead).toBeUndefined();
  });

  // The engine hands the message over at once, and the prompt it records then
  // carries the RUNNING turn's anchor. It is not a step of that turn.
  describe('when the prompt handing it over is recorded mid-turn', () => {
    const called = { type: 'CodingAgentToolCalled', name: 'Bash', args: { command: 'wait' }, tool_use_id: 't1', channel: 'claude_code' };
    const result = { type: 'CodingAgentToolResult', name: '', result: 'done', tool_use_id: 't1', channel: 'claude_code' };
    const handedOver = [
      ev(1, message('start'), 'm1'),
      ev(2, read('m1')),
      ev(3, called),
      ev(4, message('also check the totals'), 'm2'),
      ev(5, { type: 'CodingAgentPromptSent', text: 'also check the totals', channel: 'claude_code' }),
      ev(6, result),
      ev(7, read('m2')),
    ];
    const thinkingRows = (ex: Exchange) =>
      exchangeResponseEvents(ex, false).filter(e => e.type === 'step' && isThinking(e));

    it('leaves no Thinking row in the turn it waited behind', () => {
      const exchanges = groupIntoExchanges(new Map([...handedOver, ev(8, said('checking the totals'))]));
      const first = byMessage(exchanges, 'start');
      expect(thinkingRows(first)).toEqual([]);
      expect(exchangeSteps(first, false).filter(isThinking)).toEqual([]);
    });

    it('keeps the running turn\'s own prompt while it waits', () => {
      const exchanges = groupIntoExchanges(new Map([
        ...handedOver.slice(0, 5),
        ev(6, { type: 'CodingAgentPromptSent', text: '', channel: 'claude_code' }),
      ]));
      const first = byMessage(exchanges, 'start');
      expect(first.steps.filter(s => s.event.type === 'CodingAgentPromptSent')).toHaveLength(1);
    });

    it('shows the agent thinking about it once read', () => {
      const exchanges = groupIntoExchanges(new Map(handedOver));
      const second = byMessage(exchanges, 'also check the totals');
      expect(exchangeSteps(second, true).map(s => [s.description, s.outcome]))
        .toEqual([['Thinking', 'pending']]);
    });
  });

  it('takes the next turn when the running one ends with it unread', () => {
    const exchanges = groupIntoExchanges(new Map([
      ev(1, message('start'), 'm1'),
      ev(2, read('m1')),
      ev(3, said('working on it')),
      ev(4, message('also check the totals'), 'm2'),
      ev(5, done),
      ev(6, idled),
      ev(7, said('checking the totals')),
    ]));
    const first = byMessage(exchanges, 'start');
    expect(first.steps.map(s => s.event.type)).toContain('CodingAgentIdled');
    expect(texts(byMessage(exchanges, 'also check the totals'))).toEqual(['checking the totals']);
  });

  it('queues behind messages already waiting, and reads in order', () => {
    const exchanges = groupIntoExchanges(new Map([
      ...running,
      ev(6, message('and the dates'), 'm3'),
      ev(7, read('m3')),
      ev(8, said('both at once')),
    ]));
    expect(byMessage(exchanges, 'also check the totals').awaitingRead).toBeUndefined();
    expect(texts(byMessage(exchanges, 'and the dates'))).toEqual(['both at once']);
  });

  it('takes the turn when sent in history from before read events', () => {
    const exchanges = groupIntoExchanges(new Map([
      ev(1, message('start'), 'm1'),
      ev(2, said('working on it')),
      ev(3, message('also check the totals'), 'm2'),
      ev(4, said('checking the totals')),
    ]));
    expect(byMessage(exchanges, 'also check the totals').awaitingRead).toBeUndefined();
    expect(texts(byMessage(exchanges, 'also check the totals'))).toEqual(['checking the totals']);
  });

  it('keeps waiting on a page that starts after the running turn read', () => {
    const exchanges = groupIntoExchanges(new Map([
      ev(3, said('working on it')),
      ev(4, message('also check the totals'), 'm2'),
      ev(5, said('still on the first job')),
      ev(6, read('m2')),
      ev(7, said('checking the totals')),
    ]), true);
    expect(texts(byMessage(exchanges, 'also check the totals'))).toEqual(['checking the totals']);
  });

  it('stops waiting once the agent goes idle and another message arrives', () => {
    const exchanges = groupIntoExchanges(new Map([
      ev(1, message('start'), 'm1'),
      ev(2, read('m1')),
      ev(3, message('also check the totals'), 'm2'),
      ev(4, done),
      ev(5, message('next'), 'm3'),
      ev(6, said('on it')),
    ]));
    expect(byMessage(exchanges, 'also check the totals').awaitingRead).toBeUndefined();
    expect(texts(byMessage(exchanges, 'next'))).toEqual(['on it']);
  });

  it('does not wait when the agent is idle', () => {
    const exchanges = groupIntoExchanges(new Map([
      ev(1, message('start'), 'm1'),
      ev(2, read('m1')),
      ev(3, said('working on it')),
      ev(4, done),
      ev(5, message('next'), 'm2'),
      ev(6, said('on it')),
    ]));
    expect(byMessage(exchanges, 'next').awaitingRead).toBeUndefined();
    expect(texts(byMessage(exchanges, 'next'))).toEqual(['on it']);
  });

  it('renders in the queue while the running turn keeps the live stream', () => {
    const exchanges = groupIntoExchanges(new Map(running));
    const run = queuedFollowupRun(exchanges, true, true);
    expect(run.queuedOrder).toEqual([1]);
    expect(run.activeIndex).toBe(0);
  });

  it('is never retracted by a Stop, since the agent already holds it', () => {
    const exchanges = groupIntoExchanges(new Map(running));
    expect(queuedMessagesFromExchanges(exchanges, true, true)).toEqual([]);
  });

  it('queues while still on its way, before the server confirms it', () => {
    const thread = makeOptimisticThreadState({
      id: 't1',
      title: 'A coding-agent thread',
      channel: 'claude_code',
      initiator: 'user',
      eventsLoaded: true,
      pendingUserMessages: [
        { text: 'also check the totals', eventId: 'm2', created: '2026-09-24T06:14:00Z' },
      ],
    });
    for (const [seq, event] of running.slice(0, 3)) thread.events.set(seq, event);
    computeExchanges({ ...thread, pendingUserMessages: [] });
    const pending = byMessage(computeExchanges(thread), 'also check the totals');
    expect(pending.awaitingRead).toBe(true);
  });

  // The reader typed a reply while the question was open, then pressed Cancel
  // before the agent took it. The card must not jump below the reply, which
  // would leave the reply above a tall card and out of view.
  it('stays below a question card canceled while it waited', () => {
    const asked = {
      type: 'UserQuestionAsked',
      tool_use_id: 'q1',
      cc_session_id: '',
      question: 'Which look?',
      options: [{ id: 'opt-0', label: 'Soft' }],
      channel: 'claude_code',
    };
    const canceled = { type: 'UserQuestionAnswered', tool_use_id: 'q1', answer: { kind: 'Canceled' } };
    const stopped = { type: 'ResponseCanceled', cause: 'user_stop', channel: 'claude_code' };
    const thread = makeOptimisticThreadState({
      id: 't1',
      title: 'A coding-agent thread',
      channel: 'claude_code',
      initiator: 'user',
      eventsLoaded: true,
      pendingUserMessages: [
        { text: 'keep the font white', eventId: 'm2', created: '2026-09-24T06:13:04Z' },
      ],
    });
    for (const [seq, event] of [
      ev(1, message('start'), 'm1'),
      ev(2, read('m1')),
      ev(3, asked),
      ev(5, canceled),
      ev(6, stopped),
      ev(7, idled),
    ]) thread.events.set(seq, event);
    const exchanges = computeExchanges(thread);
    const card = exchanges.findIndex(ex => ex.userEvent.type === 'UserQuestionAsked');
    const reply = exchanges.indexOf(byMessage(exchanges, 'keep the font white'));
    expect(reply).toBeGreaterThan(card);
    expect(reply).toBe(exchanges.length - 1);
  });
});
