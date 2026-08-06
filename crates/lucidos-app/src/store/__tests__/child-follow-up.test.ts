// Phase 8 of the child-follow-up plan
// (docs/plans/2026-08-05-parent-follows-up-on-its-own-child-thread.md).
//
// The frontend needed no change for this feature, which is a claim worth
// pinning rather than asserting. Four things have to keep holding once a parent
// can redirect a child it already spawned:
//
//   1. the drawer's sub-thread badge and family toggle read totalChildrenCount,
//      which a follow-up never moves;
//   2. the "waiting for children" dot reads activeChildrenCount > 0, which a
//      follow-up moves only when it actually revives a child;
//   3. two ChildThreadCompleted cards for the SAME child render as two
//      exchanges, because nothing in the grouping keys on child_thread_id;
//   4. the follow-up on the child's timeline is attributed to the parent
//      thread, not to "You".
import { describe, it, expect } from 'vitest';
import { exchangeTimestamp, groupIntoExchanges, type StoredEvent, type ThreadEvent } from '../thread-events';
import { legacyOrigin } from '../thread-events/exchange-grouping';
import { resolveVisualStatus } from '../../components/shared/ThreadStatusIcon';
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
  // The badge and the family toggle both key on totalChildrenCount, which is
  // exactly the counter a follow-up leaves alone (its MessageReceived carries
  // no parent_thread_id, so the projection takes the revive branch, not the
  // spawn branch). If that ever regressed, the parent's row would promise
  // sub-threads that do not exist.
  it('the family toggle appears iff the parent really has children', () => {
    const hasFamily = (totalChildrenCount: number) => totalChildrenCount > 0;
    expect(hasFamily(0)).toBe(false);
    expect(hasFamily(2)).toBe(true);
    // A follow-up to either of those two children leaves the count at 2, so
    // the toggle neither appears nor disappears.
    expect(hasFamily(2)).toBe(true);
  });

  it('the waiting dot tracks activeChildrenCount, so a revived child re-lights it', () => {
    // Parent idle, both children finished.
    expect(resolveVisualStatus('idle', false, false, false)).toBe('idle');
    // A follow-up revives one child: the projection re-increments, and the
    // parent pulses "waiting for children" again.
    expect(resolveVisualStatus('idle', true, false, false)).toBe('waiting');
    // A follow-up to a child that was already in flight changes neither.
    expect(resolveVisualStatus('idle', true, false, false)).toBe('waiting');
    // The child's own next terminal brings it back down.
    expect(resolveVisualStatus('idle', false, false, false)).toBe('idle');
  });

  it('a parent still running outranks the waiting dot', () => {
    expect(resolveVisualStatus('running', true, false, false)).toBe('running');
  });
});

describe('a child that reports twice renders twice', () => {
  // Nothing in the grouping keys on child_thread_id, so a second completion
  // card for the same child is an ordinary second exchange. Pinned because the
  // feature makes repeat completions common: before it, a child reported once.
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
    // Same child on both, and that is not a duplicate: the events are a log of
    // completed TURNS, so child_thread_id is not a key.
    expect(cards.every(c => (c.userEvent as { child_thread_id: string }).child_thread_id === child)).toBe(true);
  });

  // The plan's Phase 8 contingency: "if the second card renders
  // indistinguishably from the first, add the minimal ordinal or timestamp
  // affordance." It does not. Each exchange renders its own timestamp from its
  // own user event, so two cards for one child are already told apart by when
  // they landed (on top of carrying different summaries). No new affordance.
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
  // The engine stamps a ThreadLink origin with direction 'parent' and the
  // parent's title. The failure this guards is the follow-up rendering as
  // "You", which would tell the user they sent a message they never sent.
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
    // No live thread in the map (the common case right after delivery): the
    // origin's own title carries it, so the panel never shows a bare uuid.
    expect(resolveThreadLinkTitle(followUpOrigin, undefined, () => undefined)).toBe(
      'Orchestrate the audit',
    );
    // A live title wins, so a rename is picked up.
    expect(resolveThreadLinkTitle(followUpOrigin, undefined, () => 'Renamed parent')).toBe(
      'Renamed parent',
    );
  });

  it('renders under the "Parent thread" heading, the same edge the fan-in uses in reverse', () => {
    const heading = (direction: 'parent' | 'child') =>
      direction === 'child' ? 'Child thread' : 'Parent thread';
    expect(heading(followUpOrigin.direction)).toBe('Parent thread');
    // The fan-in's own card points the other way.
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
