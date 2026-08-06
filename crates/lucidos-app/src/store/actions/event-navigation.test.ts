import { describe, it, expect, beforeEach, vi } from 'vitest';
import { focusedThreadId, threadMap, toasts } from '../store';
import { ApiError } from '../../api/client';
import type { ThreadState } from '../thread-events';

/**
 * "show it" on an event-wait step must always land somewhere or say why not.
 * The matched event is normally in ANOTHER thread (a wait exists precisely
 * because the thread is watching something happening elsewhere), so every
 * outcome here is a real one, not an edge case.
 */

const fetchEventLocation = vi.hoisted(() => vi.fn());
vi.mock('../../api/threads', () => ({ fetchEventLocation }));

const ensureThreadByIdInMap = vi.hoisted(() => vi.fn(async () => true));
const loadThreadEvents = vi.hoisted(() => vi.fn(async () => {}));
vi.mock('./thread-loading', () => ({ ensureThreadByIdInMap, loadThreadEvents }));

const focusThread = vi.hoisted(() => vi.fn());
vi.mock('./threads', () => ({ focusThread }));

const { showEventWhereItLives } = await import('./event-navigation');

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

describe('showEventWhereItLives', () => {
  beforeEach(() => {
    toasts.value = [];
    threadMap.value = new Map();
    focusedThreadId.value = null;
    fetchEventLocation.mockReset();
    ensureThreadByIdInMap.mockReset().mockResolvedValue(true);
    loadThreadEvents.mockReset().mockResolvedValue(undefined);
    focusThread.mockReset();
  });

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
});
