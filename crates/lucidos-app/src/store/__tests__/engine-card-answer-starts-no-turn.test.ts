/** A card the engine raised itself, such as one trigger grant review card,
 *  wakes no agent when answered. Its answer must neither move the card nor
 *  open a "Requesting" turn while the next card still waits. */
import { describe, expect, it } from 'vitest';
import { computeExchanges, type ThreadEvent } from '../thread-events';
import { exchangeStatus } from '../thread-events/exchange-render';
import { makeThreadState } from './thread-events-helpers';

const OPENED = 'evt-review-opened';
const at = (s: number) => `2026-06-01T00:00:${String(s).padStart(2, '0')}Z`;

interface CardKind { action?: string; prepared?: string }

function card(id: string, { action = 'run-batch', prepared }: CardKind): ThreadEvent {
  return {
    type: 'UserQuestionAsked',
    tool_use_id: id,
    cc_session_id: '',
    question: `Choose grants for ${id}`,
    options: [{ id: 'Bash(curl:*)', label: 'curl' }],
    multi_select: true,
    permission_request: { action, choices: {}, ...(prepared ? { prepared } : {}) },
    request_event_id: OPENED,
    channel: 'chat',
    created: at(1),
  } as unknown as ThreadEvent;
}

function reviewThread(kind: CardKind) {
  const thread = makeThreadState(new Map<number, ThreadEvent>([
    [1, { type: 'MessageReceived', text: 'Review trigger grants', channel: 'chat', _eventId: OPENED, created: at(1) } as unknown as ThreadEvent],
    [2, { type: 'ResponseGenerated', text: 'Category grants ended.', request_event_id: OPENED, channel: 'chat', created: at(1) } as unknown as ThreadEvent],
    [3, card('first', kind)],
    [4, card('second', kind)],
    [5, {
      type: 'UserQuestionAnswered',
      tool_use_id: 'first',
      answer: { kind: 'MultiSelected', option_ids: ['Bash(curl:*)'] },
      quiet: true,
      channel: 'chat',
      created: at(9),
    } as unknown as ThreadEvent],
  ]));
  thread.meta = { ...thread.meta, status: 'waiting_for_user_answer' };
  return thread;
}

function rows(kind: CardKind = {}) {
  const exchanges = computeExchanges(reviewThread(kind));
  return exchanges.map((exchange, i) => ({
    card: (exchange.userEvent as { tool_use_id?: string }).tool_use_id ?? exchange.userEvent.type,
    status: exchangeStatus(exchange, '', i === exchanges.length - 1, false, false, false, true),
  }));
}

describe('answering an engine card', () => {
  it('keeps the card in place, settled, under the card still waiting', () => {
    expect(rows()).toEqual([
      { card: 'MessageReceived', status: 'done' },
      { card: 'first', status: 'done' },
      { card: 'second', status: 'awaiting-answer' },
    ]);
  });

  it('still resumes the turn for a card an agent prepared', () => {
    expect(rows({ prepared: 'toolu-1' }).slice(-1)[0]).toEqual({ card: 'first', status: 'pending' });
  });

  it('still resumes the turn for an engine step card, whose answer runs the step', () => {
    expect(rows({ action: 'harden' }).slice(-1)[0]).toEqual({ card: 'first', status: 'pending' });
  });
});
