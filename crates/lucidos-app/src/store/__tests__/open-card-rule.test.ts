import { describe, it, expect } from 'vitest';
import { groupIntoExchanges, isOpenQuestionCard, markWaitingCards, type StoredEvent } from '../thread-events';


const created = (seq: number) => `2026-11-23T05:15:${String(seq).padStart(2, '0')}Z`;

const engineCard = (seq: number, toolUseId: string): StoredEvent => ({
  type: 'UserQuestionAsked',
  tool_use_id: toolUseId,
  cc_session_id: '',
  question: 'Wake the agent to read what arrived?',
  options: [{ id: 'a', label: 'Wake it once' }],
  permission_request: { action: 'read-waiting-reports', choices: { a: { kind: 'refuse' } } },
  created: created(seq),
} as StoredEvent);

const at = (seq: number, e: object): StoredEvent => ({ ...e, created: created(seq) } as StoredEvent);

const idled = { type: 'CodingAgentIdled', has_changes: false, is_external_repo: false, requires_restart: false, coding_agent: 'claude-code', bg_bash_pending: false };

describe('the open-card rule in the app', () => {
  it('keeps an engine card promoted mid-turn open after CodingAgentIdled', () => {
    const exchanges = groupIntoExchanges(new Map<number, StoredEvent>([
      [1, at(1, { type: 'MessageReceived', text: 'Resume' })],
      [2, engineCard(2, 'permission-request:1')],
      [3, at(3, { type: 'CodingAgentTextStreamed', text: 'The split is running again' })],
      [4, at(4, idled)],
    ]));
    const card = exchanges.find(e => e.userEvent.type === 'UserQuestionAsked')!;
    expect(card.questionOvertaken).toBe(false);
    expect(isOpenQuestionCard(card)).toBe(true);
  });

  it('closes an agent card when its turn ends', () => {
    const exchanges = groupIntoExchanges(new Map<number, StoredEvent>([
      [1, at(1, { type: 'MessageReceived', text: 'go' })],
      [2, at(2, { type: 'UserQuestionAsked', tool_use_id: 'tu', cc_session_id: 's', question: 'X?', options: [{ id: 'a', label: 'X' }] })],
      [3, at(3, idled)],
    ]));
    const card = exchanges.find(e => e.userEvent.type === 'UserQuestionAsked')!;
    expect(isOpenQuestionCard(card)).toBe(false);
  });

  it('marks every open card after the oldest as waiting', () => {
    const exchanges = groupIntoExchanges(new Map<number, StoredEvent>([
      [1, engineCard(1, 'permission-request:1')],
      [2, at(2, idled)],
      [3, engineCard(3, 'permission-request:2')],
    ]));
    const marked = markWaitingCards(exchanges);
    const cards = marked.filter(e => e.userEvent.type === 'UserQuestionAsked');
    expect(cards.map(c => c.cardWaiting === true)).toEqual([false, true]);
    expect(exchanges.some(e => e.cardWaiting)).toBe(false);
  });
});
