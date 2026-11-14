/** A message typed while a question card waits IS that card's answer: the
 *  engine routes it there as `FreeText`. Until the answer lands, it draws on
 *  the card it answers rather than as a queued row below it. */
import { describe, expect, it } from 'vitest';
import { computeExchanges, type PendingUserMessage, type ThreadEvent, type ThreadStatus } from '../thread-events';
import { makeThreadState, TS } from './thread-events-helpers';
import { pendingAnswers, questionTheSendAnswers } from '../pendingDecisions';

const asked = {
  type: 'UserQuestionAsked',
  tool_use_id: 'tu-1',
  cc_session_id: 'sess',
  question: 'Which one?',
  options: [{ id: 'a', label: 'A' }],
  created: TS,
} as ThreadEvent;

function waitingThread(status: ThreadStatus = 'waiting_for_user_answer') {
  const thread = makeThreadState(new Map<number, ThreadEvent>([
    [1, { type: 'MessageReceived', text: 'go', channel: 'chat', created: TS } as ThreadEvent],
    [2, asked],
  ]));
  thread.meta = { ...thread.meta, status };
  return thread;
}

const pending = (over: Partial<PendingUserMessage> = {}): PendingUserMessage => ({
  text: 'neither, do C',
  eventId: 'e-1',
  created: '2026-04-17T00:00:05Z',
  ...over,
});

describe('a typed answer in flight', () => {
  it('draws on its question card and opens no row of its own', () => {
    const thread = waitingThread();
    thread.pendingUserMessages = [pending({ answersQuestion: 'tu-1' })];
    const exchanges = computeExchanges(thread);
    expect(exchanges.map(e => e.userEvent.type)).toEqual(['MessageReceived', 'UserQuestionAsked']);
    expect(exchanges[1].typedAnswer).toEqual({ state: 'sending', text: 'neither, do C', image_hashes: [] });
  });

  it('falls back to a row once the safety refetch gave up on it, and the card goes live', () => {
    const thread = waitingThread();
    thread.pendingUserMessages = [pending({ answersQuestion: 'tu-1', unconfirmed: true })];
    const exchanges = computeExchanges(thread);
    expect(exchanges).toHaveLength(3);
    expect(exchanges[1].typedAnswer).toBeUndefined();
  });

  it('falls back to a row when its card is not in the thread', () => {
    const thread = waitingThread();
    thread.pendingUserMessages = [pending({ answersQuestion: 'tu-gone' })];
    const exchanges = computeExchanges(thread);
    expect(exchanges).toHaveLength(3);
    expect(exchanges[2].userEvent).toMatchObject({ type: 'MessageReceived', text: 'neither, do C' });
    expect(exchanges[1].typedAnswer).toBeUndefined();
  });

  it('falls back to a row when its card was answered some other way', () => {
    const thread = waitingThread();
    thread.events.set(3, { type: 'UserQuestionAnswered', tool_use_id: 'tu-1', answer: { kind: 'Selected', option_id: 'a' }, created: TS } as ThreadEvent);
    thread.pendingUserMessages = [pending({ answersQuestion: 'tu-1' })];
    const exchanges = computeExchanges(thread);
    expect(exchanges[1].typedAnswer).toBeUndefined();
    expect(exchanges[exchanges.length - 1].userEvent).toMatchObject({ type: 'MessageReceived', text: 'neither, do C' });
  });

  it('leaves an ordinary pending message as a row', () => {
    const thread = waitingThread();
    thread.pendingUserMessages = [pending()];
    expect(computeExchanges(thread)).toHaveLength(3);
  });
});

describe('questionTheSendAnswers', () => {
  it('names the open question while the thread waits for an answer', () => {
    expect(questionTheSendAnswers(waitingThread())).toBe('tu-1');
  });

  it('names nothing while a turn runs', () => {
    expect(questionTheSendAnswers(waitingThread('running'))).toBeUndefined();
  });

  it('names nothing behind an answer already in flight, so a second send queues', () => {
    const thread = waitingThread();
    thread.pendingUserMessages = [pending({ answersQuestion: 'tu-1' })];
    expect(questionTheSendAnswers(thread)).toBeUndefined();
  });

  it('names nothing while a newer permission card is what waits', () => {
    const thread = waitingThread();
    thread.events.set(3, {
      type: 'CodingAgentPermissionRequest',
      request_id: 'req-1',
      tool_use_id: 'tu-p',
      tool_name: 'Bash',
      input: { command: 'ls' },
      summary: 'list files',
      created: TS,
    } as ThreadEvent);
    expect(questionTheSendAnswers(thread)).toBeUndefined();
  });

  it('names nothing while a tapped option is on its way to the card', () => {
    pendingAnswers.set('tu-1', { kind: 'Selected', option_id: 'a' });
    try {
      expect(questionTheSendAnswers(waitingThread())).toBeUndefined();
    } finally {
      pendingAnswers.clear('tu-1');
    }
  });

  it('names nothing once the question is answered', () => {
    const thread = waitingThread();
    thread.events.set(3, { type: 'UserQuestionAnswered', tool_use_id: 'tu-1', answer: { kind: 'Selected', option_id: 'a' }, created: TS } as ThreadEvent);
    expect(questionTheSendAnswers(thread)).toBeUndefined();
  });
});
