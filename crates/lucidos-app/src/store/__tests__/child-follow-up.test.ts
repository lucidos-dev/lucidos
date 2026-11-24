import { describe, it, expect } from 'vitest';
import { exchangeTimestamp, groupIntoExchanges, type StoredEvent, type ThreadEvent } from '../thread-events';
import { legacyOrigin } from '../thread-events/exchange-grouping';
import { resolveVisualStatus } from '../../components/shared/threadVisualStatus';
import { resolveThreadLinkTitle } from '../../components/chat/MessageRoutePanel';
import { actorInitiator } from '../../components/chat/ChatExchange';

const PARENT = 'parent-thread-id';

function ev(seq: number, e: ThreadEvent) {
  return [seq, { ...e, created: `2026-08-05T12:00:${String(seq).padStart(2, '0')}Z` }] as const;
}
function thread(...entries: Array<readonly [number, StoredEvent]>) {
  return new Map(entries);
}
function completion(childId: string, summary: string): ThreadEvent {
  return {
    type: 'ChildThreadCompleted',
    child_thread_id: childId,
    child_thread_title: 'Audit the auth module',
    status: 'success',
    summary,
  } as ThreadEvent;
}

describe('the drawer counters a follow-up must not disturb', () => {
  it('the family toggle appears iff the parent really has children', () => {
    const hasFamily = (totalChildrenCount: number) => totalChildrenCount > 0;
    expect(hasFamily(0)).toBe(false);
    expect(hasFamily(2)).toBe(true);
    expect(hasFamily(2)).toBe(true);
  });

  it('the waiting dot tracks activeChildrenCount, so a revived child re-lights it', () => {
    expect(resolveVisualStatus('idle', false, false, false)).toBe('idle');
    expect(resolveVisualStatus('idle', true, false, false)).toBe('waiting');
    expect(resolveVisualStatus('idle', true, false, false)).toBe('waiting');
    expect(resolveVisualStatus('idle', false, false, false)).toBe('idle');
  });

  it('a parent still running outranks the waiting dot', () => {
    expect(resolveVisualStatus('running', true, false, false)).toBe('running');
  });
});

describe('a child that reports twice renders twice', () => {
  it('two ChildThreadCompleted for the same child are two exchanges', () => {
    const child = 'child-b';
    const events = thread(
      ev(1, { type: 'MessageReceived', text: 'do two things' }),
      ev(2, completion(child, 'first pass done')),
      ev(3, { type: 'ResponseGenerated', text: 'redirecting B' }),
      ev(4, completion(child, 'second pass done')),
    );
    const exchanges = groupIntoExchanges(events);

    const cards = exchanges.filter(x => x.userEvent.type === 'ChildThreadCompleted');
    expect(cards).toHaveLength(2);
    expect(cards.map(c => (c.userEvent as { summary: string }).summary)).toEqual([
      'first pass done',
      'second pass done',
    ]);
    expect(cards.every(c => (c.userEvent as { child_thread_id: string }).child_thread_id === child)).toBe(true);
  });

  it('the two cards carry distinct timestamps, so no ordinal affordance is needed', () => {
    const child = 'child-b';
    const events = thread(
      ev(1, { type: 'MessageReceived', text: 'do two things' }),
      ev(2, completion(child, 'first pass done')),
      ev(4, completion(child, 'second pass done')),
    );
    const cards = groupIntoExchanges(events).filter(x => x.userEvent.type === 'ChildThreadCompleted');
    const stamps = cards.map(exchangeTimestamp);
    expect(stamps).toHaveLength(2);
    expect(stamps[0]).not.toBe(stamps[1]);
  });

  it('completions from different children stay separate exchanges too', () => {
    const events = thread(
      ev(1, { type: 'MessageReceived', text: 'do two things' }),
      ev(2, completion('child-a', 'A done')),
      ev(3, completion('child-b', 'B done')),
    );
    const cards = groupIntoExchanges(events).filter(x => x.userEvent.type === 'ChildThreadCompleted');
    expect(cards).toHaveLength(2);
  });
});

describe("the follow-up on the child's timeline", () => {
  const followUpOrigin = {
    kind: 'thread_link',
    thread_id: PARENT,
    title: 'Orchestrate the audit',
    mode: 'agent',
    direction: 'parent',
  } as const;

  it('is attributed to the agent, never to "You"', () => {
    const { label } = actorInitiator(followUpOrigin);
    expect(label).not.toBe('You');
  });

  it('names the parent by title in the route panel', () => {
    expect(resolveThreadLinkTitle(followUpOrigin, undefined, () => undefined)).toBe(
      'Orchestrate the audit',
    );
    expect(resolveThreadLinkTitle(followUpOrigin, undefined, () => 'Renamed parent')).toBe(
      'Renamed parent',
    );
  });

  it('renders under the "Parent thread" heading, the same edge the fan-in uses in reverse', () => {
    const heading = (direction: 'parent' | 'child') =>
      direction === 'child' ? 'Child thread' : 'Parent thread';
    expect(heading(followUpOrigin.direction)).toBe('Parent thread');
    expect(heading('child')).toBe('Child thread');
  });

  it('a message with an explicit origin keeps it rather than synthesizing one', () => {
    const withOrigin = {
      type: 'MessageReceived',
      text: 'go the other way',
      mode: 'agent',
      origin: followUpOrigin,
    } as Extract<ThreadEvent, { type: 'MessageReceived' }>;
    expect(legacyOrigin(withOrigin)).toEqual(followUpOrigin);
  });
});
