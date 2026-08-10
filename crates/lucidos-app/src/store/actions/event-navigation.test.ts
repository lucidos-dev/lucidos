import { describe, it, expect, beforeEach, vi } from 'vitest';
import { focusedThreadId, threadMap, toasts } from '../store';
import { ApiError } from '../../api/client';
import type { ThreadState } from '../thread-events';

/**
 * "show it" on an event-wait step must always land somewhere or say why not.
 * The matched event is normally in ANOTHER thread (a wait exists precisely
 * because the thread is watching something happening elsewhere), so every
 * outcome here is a real one, not an edge case.
 *
 * Since 2026-08-10 the same resolution also runs BEFORE the click, so the wake
 * card and the trigger-fired row can decline to draw a jump that has nowhere to
 * go. `resolveEventTarget` is the shared half; the second describe below pins
 * the render-time question it answers.
 */

const fetchEventLocation = vi.hoisted(() => vi.fn());
vi.mock('../../api/threads', () => ({ fetchEventLocation }));

const ensureThreadByIdInMap = vi.hoisted(() => vi.fn(async () => true));
const loadThreadEvents = vi.hoisted(() => vi.fn(async () => {}));
vi.mock('./thread-loading', () => ({ ensureThreadByIdInMap, loadThreadEvents }));

const focusThread = vi.hoisted(() => vi.fn());
vi.mock('./threads', () => ({ focusThread }));

const {
  _resetEventTargetCacheForTesting,
  ensureEventTargetResolved,
  eventHasTarget,
  resolveEventTarget,
  showEventWhereItLives,
} = await import('./event-navigation');

/** A LOADED thread carrying one turn: a `MessageReceived` starter plus the steps
 *  given. Only the fields `computeExchanges` reads plus the `eventsLoaded` flag
 *  the anchor wait settles on are populated. */
function threadWith(starterId: string, steps: [string, string][]): ThreadState {
  const events = new Map<number, unknown>();
  let seq = 0;
  events.set(++seq, { type: 'MessageReceived', content: 'go', _eventId: starterId, created: '2026-08-06T10:00:00Z' });
  for (const [type, id] of steps) {
    events.set(++seq, { type, _eventId: id, created: '2026-08-06T10:00:01Z' });
  }
  return { events, pendingUserMessages: [], eventsLoaded: true } as unknown as ThreadState;
}

function toastMessages(): string[] {
  return toasts.value.map(t => t.message);
}

/** Let a fire-and-forget resolution run to completion.
 *
 *  A macrotask tick, so the whole microtask queue drains: `ensureEventTargetResolved`
 *  returns void, and every await in the chain behind it is on an already-settled
 *  mock. Waiting on the request COUNT instead would prove nothing, since the
 *  mock is called synchronously and the `.catch` that decides whether to record
 *  a verdict has not run yet. */
function settle(): Promise<void> {
  return new Promise(r => setTimeout(r, 0));
}

function resetAll(): void {
  toasts.value = [];
  threadMap.value = new Map();
  focusedThreadId.value = null;
  fetchEventLocation.mockReset();
  ensureThreadByIdInMap.mockReset().mockResolvedValue(true);
  loadThreadEvents.mockReset().mockResolvedValue(undefined);
  focusThread.mockReset();
  _resetEventTargetCacheForTesting();
}

describe('showEventWhereItLives', () => {
  beforeEach(resetAll);

  it('navigates to the owning thread, re-targeted at the turn holding the event', async () => {
    fetchEventLocation.mockResolvedValue({ thread_id: 'other' });
    threadMap.value = new Map([
      ['other', threadWith('starter-1', [['CodingAgentIdled', 'idle-1']])],
    ]);

    await showEventWhereItLives('idle-1');

    expect(fetchEventLocation).toHaveBeenCalledWith('idle-1');
    // `CodingAgentIdled` stamps no element of its own, so the deep-link has to
    // aim at the turn that contains it or it would never resolve.
    expect(focusThread).toHaveBeenCalledWith('other', { targetEventId: 'starter-1' });
    expect(toastMessages()).toEqual([]);
  });

  /** A wait can match an event in its own thread. On an iOS PWA over Tailscale a
   *  round-trip is long enough for the tap to read as dead, so the open thread's
   *  already-loaded events answer first. */
  it('skips the round-trip when the event is in the open thread', async () => {
    threadMap.value = new Map([
      ['open', threadWith('starter-1', [['CodingAgentIdled', 'idle-1']])],
    ]);
    focusedThreadId.value = 'open';

    await showEventWhereItLives('idle-1');

    expect(fetchEventLocation).not.toHaveBeenCalled();
    expect(focusThread).toHaveBeenCalledWith('open', { targetEventId: 'starter-1' });
  });

  /** A workspace domain event renders in no transcript at all. That is a real
   *  answer from the engine (`thread_id: null`), and it must not read the same
   *  as an event that has gone missing. */
  it('says a workspace event belongs to no conversation', async () => {
    fetchEventLocation.mockResolvedValue({ thread_id: null });

    await showEventWhereItLives('domain-1');

    expect(focusThread).not.toHaveBeenCalled();
    expect(toastMessages()[0]).toContain('workspace event');
  });

  it('distinguishes an event that is no longer in the event store', async () => {
    fetchEventLocation.mockRejectedValue(new ApiError(404, 'Event not found'));

    await showEventWhereItLives('gone-1');

    expect(focusThread).not.toHaveBeenCalled();
    expect(toastMessages()[0]).toContain('no longer in the event store');
    expect(toastMessages()[0]).not.toContain('workspace event');
  });

  it('reports a transport failure instead of failing silently', async () => {
    fetchEventLocation.mockRejectedValue(new TypeError('Load failed'));

    await showEventWhereItLives('evt-1');

    expect(focusThread).not.toHaveBeenCalled();
    expect(toastMessages()[0]).toContain('Could not open that event');
  });

  it('reports a thread that no longer exists', async () => {
    fetchEventLocation.mockResolvedValue({ thread_id: 'archived-away' });
    ensureThreadByIdInMap.mockResolvedValue(false);

    await showEventWhereItLives('evt-1');

    expect(focusThread).not.toHaveBeenCalled();
    expect(toastMessages()[0]).toContain('no longer exists');
  });

  /** The events may still be arriving when the anchor is computed. Handing the
   *  raw id to `scrollToEventAndPulse` lets its own resolve deadline and
   *  reporting take over, rather than dropping the navigation here. */
  it('falls back to the raw event id when no turn claims it yet', async () => {
    fetchEventLocation.mockResolvedValue({ thread_id: 'other' });
    threadMap.value = new Map([['other', threadWith('starter-1', [])]]);

    await showEventWhereItLives('not-here-yet');

    expect(focusThread).toHaveBeenCalledWith('other', { targetEventId: 'not-here-yet' });
  });

  /** `loadThreadEvents` does NOT join an in-flight load: it early-returns on
   *  its `loadingThreads` guard. So while `loadAllThreads`'s eager pass is
   *  already fetching this thread, awaiting it resolves instantly against an
   *  empty event map, the anchor comes back null, and the raw id is exactly what
   *  cannot resolve for an unstamped event. The wait is on the STORE, not on
   *  that call. */
  it('waits for an already-in-flight events load before resolving the anchor', async () => {
    fetchEventLocation.mockResolvedValue({ thread_id: 'other' });
    // The eager pass has claimed the thread, so loadThreadEvents no-ops and the
    // map still holds no events for it.
    threadMap.value = new Map([
      ['other', { events: new Map(), pendingUserMessages: [] } as unknown as ThreadState],
    ]);
    loadThreadEvents.mockResolvedValue(undefined);

    const navigation = showEventWhereItLives('idle-1');
    // A macrotask tick, so the whole microtask queue drains first: every await
    // in the chain is on an already-resolved mock, so a version that waits on
    // the CALL rather than the store has navigated by now.
    await new Promise(r => setTimeout(r, 0));
    expect(focusThread).not.toHaveBeenCalled();

    // The in-flight fetch lands.
    threadMap.value = new Map([
      ['other', threadWith('starter-1', [['CodingAgentIdled', 'idle-1']])],
    ]);

    await navigation;
    expect(focusThread).toHaveBeenCalledWith('other', { targetEventId: 'starter-1' });
  });

  /** A load that failed settles the wait immediately rather than burning the
   *  whole deadline on events that are not coming. */
  it('stops waiting when the events load has failed', async () => {
    fetchEventLocation.mockResolvedValue({ thread_id: 'other' });
    threadMap.value = new Map([
      ['other', {
        events: new Map(),
        pendingUserMessages: [],
        eventsLoadFailed: true,
      } as unknown as ThreadState],
    ]);

    await showEventWhereItLives('idle-1');

    expect(focusThread).toHaveBeenCalledWith('other', { targetEventId: 'idle-1' });
  });

  it('does nothing at all for an empty event id', async () => {
    await showEventWhereItLives('');
    expect(fetchEventLocation).not.toHaveBeenCalled();
    expect(focusThread).not.toHaveBeenCalled();
    expect(toastMessages()).toEqual([]);
  });

  /** **The bug the whole change exists for**, on the click path. A background
   *  bash task completed while a `UserQuestionAsked` turn was open, so grouping
   *  filed the completion under that question and the jump pulsed it: a card
   *  with no causal relationship to the event whatsoever.
   *
   *  Now that the containing turn is not a lie the link can tell, the answer is
   *  a toast rather than a navigation. The row does not draw the jump at all
   *  (see `eventHasTarget` below), so this path is only reachable by a click
   *  racing the answer. */
  it('refuses to pulse a turn that merely happened to be open', async () => {
    threadMap.value = new Map([
      ['open', threadWith('question-1', [['BackgroundBashCompleted', 'bash-1']])],
    ]);
    focusedThreadId.value = 'open';

    await showEventWhereItLives('bash-1');

    expect(focusThread).not.toHaveBeenCalled();
    expect(toastMessages()[0]).toContain('not drawn anywhere');
  });

  /** **The click is where the render path's optimism gets corrected.**
   *
   *  A row cannot afford to load another thread just to decide whether to draw
   *  a link, so an event in an unloaded thread is drawn as linkable. That guess
   *  can be wrong, and the click is what finds out. Leaving the verdict at true
   *  would strand the chip as a link that toasts on every single tap for the
   *  rest of the session, which is a worse dead affordance than the one this
   *  whole change removed. */
  it('retires the affordance when the load proves there is nothing to land on', async () => {
    fetchEventLocation.mockResolvedValue({ thread_id: 'other' });
    ensureEventTargetResolved('bash-1');
    await settle();
    // Optimistic: the thread was not loaded, so the row drew the link.
    expect(eventHasTarget('bash-1')).toBe(true);

    // The click loads it, and the event turns out to be unanchorable.
    threadMap.value = new Map([
      ['other', threadWith('question-1', [['BackgroundBashCompleted', 'bash-1']])],
    ]);
    await showEventWhereItLives('bash-1');

    expect(toastMessages()[0]).toContain('not drawn anywhere');
    expect(eventHasTarget('bash-1')).toBe(false);
  });

  /** Same correction for a thread that has since gone: a link into it can never
   *  land again. */
  it('retires the affordance when the owning thread no longer exists', async () => {
    fetchEventLocation.mockResolvedValue({ thread_id: 'archived-away' });
    ensureEventTargetResolved('evt-1');
    await settle();
    expect(eventHasTarget('evt-1')).toBe(true);

    ensureThreadByIdInMap.mockResolvedValue(false);
    await showEventWhereItLives('evt-1');

    expect(toastMessages()[0]).toContain('no longer exists');
    expect(eventHasTarget('evt-1')).toBe(false);
  });

  /** A dropped connection is NOT an answer about the event, so the link stays
   *  and the next tap can succeed. */
  it('keeps the affordance after a transport failure', async () => {
    fetchEventLocation.mockResolvedValue({ thread_id: 'other' });
    ensureEventTargetResolved('idle-1');
    await settle();
    expect(eventHasTarget('idle-1')).toBe(true);

    ensureThreadByIdInMap.mockRejectedValue(new TypeError('Load failed'));
    await showEventWhereItLives('idle-1');

    expect(toastMessages()[0]).toContain('Could not open that event');
    expect(eventHasTarget('idle-1')).toBe(true);
  });
});

/** **Is there anywhere to go?**, asked while RENDERING rather than on the click.
 *
 *  The row draws its jump only when the answer is yes, so the three ways it can
 *  be no each have to be reachable cheaply. The expensive half is deliberately
 *  out of scope: loading another thread's events to find the exact element stays
 *  on the click, or a transcript would pay a thread load per row. */
describe('resolveEventTarget', () => {
  beforeEach(resetAll);

  /** The open thread answers without a round trip. On an iOS PWA over Tailscale
   *  that is 400 to 800 ms saved, which is the difference between a row that
   *  paints with its link and one that visibly grows it. */
  it('answers from the open thread without asking the engine', async () => {
    threadMap.value = new Map([
      ['open', threadWith('starter-1', [['CodingAgentIdled', 'idle-1']])],
    ]);
    focusedThreadId.value = 'open';

    expect(await resolveEventTarget('idle-1')).toEqual({
      kind: 'anchored', threadId: 'open', anchor: 'starter-1',
    });
    expect(fetchEventLocation).not.toHaveBeenCalled();
  });

  it('reports nowhere for an event that merely landed in the open turn', async () => {
    threadMap.value = new Map([
      ['open', threadWith('question-1', [['BackgroundBashCompleted', 'bash-1']])],
    ]);
    focusedThreadId.value = 'open';

    expect((await resolveEventTarget('bash-1')).kind).toBe('nowhere');
    expect(fetchEventLocation).not.toHaveBeenCalled();
  });

  /** A trigger fires on a workspace domain event, which belongs to no
   *  conversation. The everyday case on the trigger-fired row, and previously a
   *  guaranteed toast. */
  it('reports nowhere for an event that belongs to no conversation', async () => {
    fetchEventLocation.mockResolvedValue({ thread_id: null });

    const target = await resolveEventTarget('domain-1');
    expect(target.kind).toBe('nowhere');
    if (target.kind === 'nowhere') expect(target.note).toContain('workspace event');
  });

  /** The common wake: the wait matched a `CodingAgentIdled` in the coding-agent
   *  thread it was watching. There IS somewhere to go; WHERE needs that thread's
   *  events, and only the click pays for those. */
  it('reports another thread as somewhere to go, without loading it', async () => {
    fetchEventLocation.mockResolvedValue({ thread_id: 'other' });

    expect(await resolveEventTarget('idle-1')).toEqual({ kind: 'unloaded', threadId: 'other' });
    expect(loadThreadEvents).not.toHaveBeenCalled();
  });

  /** A thread row with no events makes every anchor trivially null, so reading
   *  one as "nowhere" would drop a working link. */
  it('does not mistake an unloaded thread for an unanchorable one', async () => {
    fetchEventLocation.mockResolvedValue({ thread_id: 'other' });
    threadMap.value = new Map([
      ['other', { events: new Map(), pendingUserMessages: [] } as unknown as ThreadState],
    ]);

    expect(await resolveEventTarget('idle-1')).toEqual({ kind: 'unloaded', threadId: 'other' });
  });

  /** Same trap one race later. `GET /events/:id/location` reads the event store,
   *  so it knows about an event a thread snapshot taken moments earlier missed
   *  and SSE has not delivered yet. That is "not here YET", not "drawn
   *  nowhere". */
  it('does not mistake an event the thread has not caught up with', async () => {
    fetchEventLocation.mockResolvedValue({ thread_id: 'other' });
    threadMap.value = new Map([['other', threadWith('starter-1', [])]]);

    expect(await resolveEventTarget('just-landed')).toEqual({ kind: 'unloaded', threadId: 'other' });
  });

  /** N rows asking about one event make ONE request. Both paths are covered:
   *  the in-flight map (a wake card and a trigger row painting together) and the
   *  resolved cache (a re-render). */
  it('asks the engine once however many rows ask', async () => {
    fetchEventLocation.mockResolvedValue({ thread_id: 'other' });

    await Promise.all([
      resolveEventTarget('shared-1'),
      resolveEventTarget('shared-1'),
      resolveEventTarget('shared-1'),
    ]);
    await resolveEventTarget('shared-1');

    expect(fetchEventLocation).toHaveBeenCalledTimes(1);
  });

  /** A dropped request caches nothing, so the answer is retried rather than
   *  frozen at whatever a flaky connection said. */
  it('retries after a transport failure instead of caching it', async () => {
    fetchEventLocation.mockRejectedValueOnce(new TypeError('Load failed'));
    await expect(resolveEventTarget('flaky-1')).rejects.toThrow('Load failed');

    fetchEventLocation.mockResolvedValue({ thread_id: 'other' });
    expect(await resolveEventTarget('flaky-1')).toEqual({ kind: 'unloaded', threadId: 'other' });
    expect(fetchEventLocation).toHaveBeenCalledTimes(2);
  });
});

/** The verdict the rows actually read. One boolean, settled once per event id,
 *  published on its own signal so a transcript row subscribes to that and not to
 *  `threadMap` (which changes on every event of every thread). */
describe('eventHasTarget', () => {
  beforeEach(resetAll);

  /** **The no-flash rule.** Unknown reads as false, so a row paints plain and
   *  GAINS its link. The other direction, a link that appears and then
   *  vanishes, reads as a bug and is tappable in the window before it goes. */
  it('is false while the answer is still unknown', () => {
    expect(eventHasTarget('never-asked')).toBe(false);
  });

  /** A delivery whose `event_id` the engine never recorded, and a scheduled
   *  trigger, both arrive here as `undefined`. Nothing is asked, and the row
   *  draws no jump. */
  it('is false for an event id that was never recorded, and asks nothing', () => {
    expect(eventHasTarget(undefined)).toBe(false);
    ensureEventTargetResolved(undefined);
    expect(fetchEventLocation).not.toHaveBeenCalled();
  });

  it('turns true once the event is found to live somewhere', async () => {
    fetchEventLocation.mockResolvedValue({ thread_id: 'other' });

    ensureEventTargetResolved('idle-1');
    expect(eventHasTarget('idle-1')).toBe(false);
    await vi.waitFor(() => expect(eventHasTarget('idle-1')).toBe(true));
  });

  it('stays false for an event belonging to no conversation', async () => {
    fetchEventLocation.mockResolvedValue({ thread_id: null });

    ensureEventTargetResolved('domain-1');
    await vi.waitFor(() => expect(fetchEventLocation).toHaveBeenCalled());
    expect(eventHasTarget('domain-1')).toBe(false);
  });

  /** The reported case, as the row sees it: no link at all on the wake card for
   *  a `BackgroundBashCompleted` that this thread cannot address. */
  it('stays false for a same-thread event with no anchor', async () => {
    threadMap.value = new Map([
      ['open', threadWith('question-1', [['BackgroundBashCompleted', 'bash-1']])],
    ]);
    focusedThreadId.value = 'open';

    ensureEventTargetResolved('bash-1');
    await vi.waitFor(() => expect(eventHasTarget('bash-1')).toBe(false));
    expect(fetchEventLocation).not.toHaveBeenCalled();
  });

  /** A 404 is a definite answer: the event has left the event store, so the
   *  verdict SETTLES rather than being retried forever.
   *
   *  "Settled" is what the second half asserts, and it needs the flush to mean
   *  anything: an unresolved verdict and a settled false both read as no link,
   *  and both suppress a second request while the first is still in flight. What
   *  tells them apart is asking again AFTER the chain has run. */
  it('settles false on a 404', async () => {
    fetchEventLocation.mockRejectedValue(new ApiError(404, 'Event not found'));

    ensureEventTargetResolved('gone-1');
    await settle();
    expect(eventHasTarget('gone-1')).toBe(false);

    ensureEventTargetResolved('gone-1');
    await settle();
    expect(fetchEventLocation).toHaveBeenCalledTimes(1);
  });

  /** A transport failure is NOT an answer. Caching it would silently remove a
   *  working affordance for the rest of the session, so the verdict is left
   *  unresolved and the next mount retries. */
  it('leaves the verdict unresolved after a transport failure', async () => {
    fetchEventLocation.mockRejectedValue(new TypeError('Load failed'));

    ensureEventTargetResolved('flaky-1');
    await settle();
    expect(eventHasTarget('flaky-1')).toBe(false);

    fetchEventLocation.mockResolvedValue({ thread_id: 'other' });
    ensureEventTargetResolved('flaky-1');
    await settle();
    expect(eventHasTarget('flaky-1')).toBe(true);
    expect(fetchEventLocation).toHaveBeenCalledTimes(2);
  });
});
