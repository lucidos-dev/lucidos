import { beforeEach, describe, expect, it } from 'vitest';
import { getExchanges, insertEvents, makeThread, resetSeqCounter } from './thread-flows-helpers';
import type { ThreadEvent } from '../thread-events';

beforeEach(resetSeqCounter);

const card = (type: 'UserQuestionAsked' | 'UserQuestionQueued', toolUseId: string) => ({
  type,
  tool_use_id: toolUseId,
  cc_session_id: '',
  question: `Question ${toolUseId}?`,
  multi_select: false,
}) as ThreadEvent;

describe('a queued card', () => {
  it('changes nothing the transcript draws', () => {
    const turn = [
      { type: 'MessageReceived', text: 'tidy the release notes' },
      card('UserQuestionAsked', 'a'),
    ] as ThreadEvent[];
    const plain = makeThread('plain', 'waiting');
    insertEvents(plain.map, plain.id, turn);
    const queued = makeThread('queued', 'waiting');
    insertEvents(queued.map, queued.id, [...turn, card('UserQuestionQueued', 'b')]);

    const drawn = (map: typeof plain.map, id: string) =>
      getExchanges(map, id).map((e) => [e.userEvent.type, e.steps.map((s) => s.event.type)]);
    expect(drawn(queued.map, queued.id)).toEqual(drawn(plain.map, plain.id));
    const everyEvent = getExchanges(queued.map, queued.id)
      .flatMap((e) => [e.userEvent, ...e.steps.map((s) => s.event)]);
    expect(everyEvent.some((e) => e.type === 'UserQuestionQueued')).toBe(false);
  });
});
