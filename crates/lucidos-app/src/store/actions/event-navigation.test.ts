import { describe, it, expect, beforeEach, vi } from 'vitest';
import { signal } from '@preact/signals';
import { focusedThreadId, setDrawerGrouping, setSelectedOngoingGroup, threadMap, threadSearchQuery, toasts } from '../store';
import { ApiError } from '../../api/client';
import type { ThreadState } from '../thread-events';


const fetchEventLocation = vi.hoisted(() => vi.fn());
vi.mock('../../api/threads', () => ({ fetchEventLocation }));

const ensureThreadByIdInMap = vi.hoisted(() => vi.fn(async () => true));
const loadThreadEvents = vi.hoisted(() => vi.fn(async () => {}));
const ensureWholeThreadLoaded = vi.hoisted(() => vi.fn(async () => {}));
const threadEventsStillArriving = vi.hoisted(() => vi.fn((_threadId: string) => false));
vi.mock('./thread-loading', () => ({
  ensureThreadByIdInMap,
  ensureWholeThreadLoaded,
  loadThreadEvents,
  threadEventsStillArriving,
}));

const focusThread = vi.hoisted(() => vi.fn());
const landOnEvent = vi.hoisted(() => vi.fn());
vi.mock('./threads', () => ({ focusThread, landOnEvent }));

const paneOnScreen = signal(true);
vi.mock('./seen-target', () => ({ threadPaneOnScreen: () => paneOnScreen.value }));

const {
  _resetEventTargetCacheForTesting,
  ensureEventTargetResolved,
  eventHasTarget,
  jumpableEventId,
  openListedThread,
  openOngoingThread,
  resolveEventTarget,
  showEventWhereItLives,
} = await import('./event-navigation');

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

function settle(): Promise<void> {
  return new Promise(r => setTimeout(r, 0));
}

function resetAll(): void {
  localStorage.clear();
  toasts.value = [];
  threadMap.value = new Map();
  focusedThreadId.value = null;
  fetchEventLocation.mockReset();
  ensureThreadByIdInMap.mockReset().mockResolvedValue(true);
  loadThreadEvents.mockReset().mockResolvedValue(undefined);
  ensureWholeThreadLoaded.mockReset().mockResolvedValue(undefined);
  threadEventsStillArriving.mockReset().mockImplementation((id: string) => {
    const t = threadMap.value.get(id);
    return !!t && !t.eventsLoaded && !t.eventsLoadFailed;
  });
  focusThread.mockReset();
  landOnEvent.mockReset();
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
    expect(focusThread).toHaveBeenCalledWith('other', { targetEventId: 'starter-1' });
    expect(toastMessages()).toEqual([]);
  });

  it('loads the rest of a paged thread when the target is behind its page', async () => {
    fetchEventLocation.mockResolvedValue({ thread_id: 'other' });
    const paged = threadWith('starter-1', []);
    paged.hasOlderEvents = true;
    threadMap.value = new Map([['other', paged]]);
    ensureWholeThreadLoaded.mockImplementation(async () => {
      threadMap.value = new Map([
        ['other', threadWith('starter-1', [['CodingAgentIdled', 'idle-1']])],
      ]);
    });

    await showEventWhereItLives('idle-1');

    expect(ensureWholeThreadLoaded).toHaveBeenCalledWith('other');
    expect(focusThread).toHaveBeenCalledWith('other', { targetEventId: 'starter-1' });
    expect(toastMessages()).toEqual([]);
  });

  it('does NOT load the whole history when the first page already answers', async () => {
    fetchEventLocation.mockResolvedValue({ thread_id: 'other' });
    threadMap.value = new Map([
      ['other', threadWith('starter-1', [['CodingAgentIdled', 'idle-1']])],
    ]);

    await showEventWhereItLives('idle-1');

    expect(ensureWholeThreadLoaded).not.toHaveBeenCalled();
  });

  it('skips the round-trip when the event is in the open thread', async () => {
    threadMap.value = new Map([
      ['open', threadWith('starter-1', [['CodingAgentIdled', 'idle-1']])],
    ]);
    focusedThreadId.value = 'open';

    await showEventWhereItLives('idle-1');

    expect(fetchEventLocation).not.toHaveBeenCalled();
    expect(focusThread).toHaveBeenCalledWith('open', { targetEventId: 'starter-1' });
  });

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

  it('falls back to the raw event id when no turn claims it yet', async () => {
    fetchEventLocation.mockResolvedValue({ thread_id: 'other' });
    threadMap.value = new Map([['other', threadWith('starter-1', [])]]);

    await showEventWhereItLives('not-here-yet');

    expect(focusThread).toHaveBeenCalledWith('other', { targetEventId: 'not-here-yet' });
  });

  it('waits for an already-in-flight events load before resolving the anchor', async () => {
    fetchEventLocation.mockResolvedValue({ thread_id: 'other' });
    threadMap.value = new Map([
      ['other', { events: new Map(), pendingUserMessages: [] } as unknown as ThreadState],
    ]);
    loadThreadEvents.mockResolvedValue(undefined);

    const navigation = showEventWhereItLives('idle-1');
    await new Promise(r => setTimeout(r, 0));
    expect(focusThread).not.toHaveBeenCalled();

    threadMap.value = new Map([
      ['other', threadWith('starter-1', [['CodingAgentIdled', 'idle-1']])],
    ]);

    await navigation;
    expect(focusThread).toHaveBeenCalledWith('other', { targetEventId: 'starter-1' });
  });

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

  it('refuses to pulse a turn that merely happened to be open', async () => {
    threadMap.value = new Map([
      ['open', threadWith('question-1', [['BackgroundBashCompleted', 'bash-1']])],
    ]);
    focusedThreadId.value = 'open';

    await showEventWhereItLives('bash-1');

    expect(focusThread).not.toHaveBeenCalled();
    expect(toastMessages()[0]).toContain('not drawn anywhere');
  });

  it('retires the affordance when the load proves there is nothing to land on', async () => {
    fetchEventLocation.mockResolvedValue({ thread_id: 'other' });
    ensureEventTargetResolved('bash-1');
    await settle();
    expect(eventHasTarget('bash-1')).toBe(true);

    threadMap.value = new Map([
      ['other', threadWith('question-1', [['BackgroundBashCompleted', 'bash-1']])],
    ]);
    await showEventWhereItLives('bash-1');

    expect(toastMessages()[0]).toContain('not drawn anywhere');
    expect(eventHasTarget('bash-1')).toBe(false);
  });

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

describe('resolveEventTarget', () => {
  beforeEach(resetAll);

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

  it('reports nowhere for an event that belongs to no conversation', async () => {
    fetchEventLocation.mockResolvedValue({ thread_id: null });

    const target = await resolveEventTarget('domain-1');
    expect(target.kind).toBe('nowhere');
    if (target.kind === 'nowhere') expect(target.note).toContain('workspace event');
  });

  it('reports another thread as somewhere to go, without loading it', async () => {
    fetchEventLocation.mockResolvedValue({ thread_id: 'other' });

    expect(await resolveEventTarget('idle-1')).toEqual({ kind: 'unloaded', threadId: 'other' });
    expect(loadThreadEvents).not.toHaveBeenCalled();
  });

  it('does not mistake an unloaded thread for an unanchorable one', async () => {
    fetchEventLocation.mockResolvedValue({ thread_id: 'other' });
    threadMap.value = new Map([
      ['other', { events: new Map(), pendingUserMessages: [] } as unknown as ThreadState],
    ]);

    expect(await resolveEventTarget('idle-1')).toEqual({ kind: 'unloaded', threadId: 'other' });
  });

  it('does not mistake an event the thread has not caught up with', async () => {
    fetchEventLocation.mockResolvedValue({ thread_id: 'other' });
    threadMap.value = new Map([['other', threadWith('starter-1', [])]]);

    expect(await resolveEventTarget('just-landed')).toEqual({ kind: 'unloaded', threadId: 'other' });
  });

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

  it('retries after a transport failure instead of caching it', async () => {
    fetchEventLocation.mockRejectedValueOnce(new TypeError('Load failed'));
    await expect(resolveEventTarget('flaky-1')).rejects.toThrow('Load failed');

    fetchEventLocation.mockResolvedValue({ thread_id: 'other' });
    expect(await resolveEventTarget('flaky-1')).toEqual({ kind: 'unloaded', threadId: 'other' });
    expect(fetchEventLocation).toHaveBeenCalledTimes(2);
  });
});

describe('eventHasTarget', () => {
  beforeEach(resetAll);

  it('is false while the answer is still unknown', () => {
    expect(eventHasTarget('never-asked')).toBe(false);
  });

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

  it('stays false for a same-thread event with no anchor', async () => {
    threadMap.value = new Map([
      ['open', threadWith('question-1', [['BackgroundBashCompleted', 'bash-1']])],
    ]);
    focusedThreadId.value = 'open';

    ensureEventTargetResolved('bash-1');
    await vi.waitFor(() => expect(eventHasTarget('bash-1')).toBe(false));
    expect(fetchEventLocation).not.toHaveBeenCalled();
  });

  it('settles false on a 404', async () => {
    fetchEventLocation.mockRejectedValue(new ApiError(404, 'Event not found'));

    ensureEventTargetResolved('gone-1');
    await settle();
    expect(eventHasTarget('gone-1')).toBe(false);

    ensureEventTargetResolved('gone-1');
    await settle();
    expect(fetchEventLocation).toHaveBeenCalledTimes(1);
  });

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

describe('openOngoingThread from Blocked', () => {
  beforeEach(resetAll);

  function waitingThread(): ThreadState {
    const events = new Map<number, unknown>([
      [1, { type: 'MessageReceived', content: 'go', _eventId: 'start-1', created: '2026-08-06T10:00:00Z' }],
      [2, { type: 'UserQuestionAsked', tool_use_id: 'tu-1', question: 'Which?', _eventId: 'q-1', created: '2026-08-06T10:00:01Z' }],
    ]);
    return {
      meta: { id: 'waiting', status: 'waiting_for_user_answer' },
      events,
      pendingUserMessages: [],
      eventsLoaded: true,
    } as unknown as ThreadState;
  }

  it.each(['live-edge', 'anchor:-20:q-1', 'line:-20:-100:q-1:1~ffffffff', 'row:-20:q-1'])(
    'restores a reached Blocked target for %s', saved => {
      threadMap.value = new Map([['waiting', waitingThread()]]);
      localStorage.setItem('lucidos-scroll-thread-waiting', saved);
      openOngoingThread('waiting', 'blocked');
      expect(focusThread).toHaveBeenCalledWith('waiting');
      expect(landOnEvent).not.toHaveBeenCalled();
    },
  );

  it.each(['start-1', 'unknown'])('lands for an unreached row: %s', rowEventId => {
    threadMap.value = new Map([['waiting', waitingThread()]]);
    localStorage.setItem('lucidos-scroll-thread-waiting', `row:-20:${rowEventId}`);
    openOngoingThread('waiting', 'blocked');
    expect(landOnEvent).toHaveBeenCalledWith('waiting', 'q-1');
  });

  it('restores a saved failure-step row in the target turn', () => {
    const waiting = waitingThread();
    const failed: ThreadState = { ...waiting, meta: { ...waiting.meta, status: 'failed' } };
    failed.events.set(2, { type: 'ResponseFailed', error: 'failed', _eventId: 'failure' } as never);
    threadMap.value = new Map([['waiting', failed]]);
    localStorage.setItem('lucidos-scroll-thread-waiting', 'row:-20:failure');
    openOngoingThread('waiting', 'blocked');
    expect(landOnEvent).not.toHaveBeenCalled();
  });

  it('lands on the open question at once when the events are loaded', () => {
    threadMap.value = new Map([['waiting', waitingThread()]]);

    openOngoingThread('waiting', 'blocked');

    expect(focusThread).toHaveBeenCalledWith('waiting');
    expect(landOnEvent).toHaveBeenCalledWith('waiting', 'q-1');
  });

  it('waits out a catch-up refresh before choosing where to land', async () => {
    const stale = waitingThread();
    stale.events.delete(2);
    threadMap.value = new Map([['waiting', stale]]);
    let refreshing = true;
    threadEventsStillArriving.mockImplementation(() => refreshing);

    openOngoingThread('waiting', 'blocked');
    focusedThreadId.value = 'waiting';
    await settle();
    expect(landOnEvent).not.toHaveBeenCalled();

    threadMap.value = new Map([['waiting', waitingThread()]]);
    refreshing = false;
    await vi.waitFor(() => expect(landOnEvent).toHaveBeenCalledWith('waiting', 'q-1'));
  });

  it('opens at once and lands once the events arrive', async () => {
    threadMap.value = new Map([
      ['waiting', { ...waitingThread(), events: new Map(), eventsLoaded: false } as unknown as ThreadState],
    ]);

    openOngoingThread('waiting', 'blocked');
    expect(focusThread).toHaveBeenCalledWith('waiting');
    focusedThreadId.value = 'waiting';

    threadMap.value = new Map([['waiting', waitingThread()]]);
    await vi.waitFor(() => expect(landOnEvent).toHaveBeenCalledWith('waiting', 'q-1'));
  });

  it('loads older history when the waiting card is not on the loaded page', async () => {
    const paged = waitingThread();
    paged.events.delete(2);
    paged.hasOlderEvents = true;
    threadMap.value = new Map([['waiting', paged]]);
    ensureWholeThreadLoaded.mockImplementation(async () => {
      threadMap.value = new Map([['waiting', waitingThread()]]);
    });

    focusedThreadId.value = 'waiting';
    openOngoingThread('waiting', 'blocked');

    await vi.waitFor(() => expect(landOnEvent).toHaveBeenCalledWith('waiting', 'q-1'));
    expect(ensureWholeThreadLoaded).toHaveBeenCalledWith('waiting');
  });

  it('does not land when the events are still arriving at the deadline', async () => {
    vi.useFakeTimers();
    try {
      threadMap.value = new Map([['waiting', waitingThread()]]);
      threadEventsStillArriving.mockImplementation(() => true);

      focusedThreadId.value = 'waiting';
      openOngoingThread('waiting', 'blocked');
      await vi.runAllTimersAsync();

      expect(landOnEvent).not.toHaveBeenCalled();
    } finally {
      vi.useRealTimers();
    }
  });

  it('skips the late landing when the user opened another thread meanwhile', async () => {
    threadMap.value = new Map([
      ['waiting', { ...waitingThread(), events: new Map(), eventsLoaded: false } as unknown as ThreadState],
    ]);

    openOngoingThread('waiting', 'blocked');
    focusedThreadId.value = 'elsewhere';
    threadMap.value = new Map([['waiting', waitingThread()]]);
    await settle();

    expect(landOnEvent).not.toHaveBeenCalled();
  });
});

describe('openListedThread', () => {
  beforeEach(() => {
    resetAll();
    paneOnScreen.value = true;
    threadMap.value = new Map([['waiting', {
      meta: { id: 'waiting', status: 'waiting_for_user_answer' },
      events: new Map<number, unknown>([
        [1, { type: 'MessageReceived', content: 'go', _eventId: 'start-1', created: '2026-08-06T10:00:00Z' }],
        [2, { type: 'UserQuestionAsked', tool_use_id: 'tu-1', question: 'Which?', _eventId: 'q-1', created: '2026-08-06T10:00:01Z' }],
      ]),
      pendingUserMessages: [],
      eventsLoaded: true,
    } as unknown as ThreadState]]);
    setDrawerGrouping('ongoing');
    setSelectedOngoingGroup('blocked');
    threadSearchQuery.value = '';
  });

  it('lands a Blocked row on its card, keeping the hand-off unrevealed', () => {
    openListedThread('waiting', { revealPane: false });

    expect(focusThread).toHaveBeenCalledWith('waiting', { revealPane: false });
    expect(landOnEvent).toHaveBeenCalledWith('waiting', 'q-1');
  });

  it('holds a hand-off behind a hidden pane until the pane is on screen', () => {
    paneOnScreen.value = false;
    focusThread.mockImplementation((id: string) => { focusedThreadId.value = id; });
    openListedThread('waiting', { revealPane: false });
    expect(landOnEvent).not.toHaveBeenCalled();

    paneOnScreen.value = true;
    expect(landOnEvent).toHaveBeenCalledWith('waiting', 'q-1');
  });

  it('drops a held hand-off once the reader opens another thread', async () => {
    paneOnScreen.value = false;
    focusThread.mockImplementation((id: string) => { focusedThreadId.value = id; });
    openListedThread('waiting', { revealPane: false });

    focusedThreadId.value = 'elsewhere';
    await settle();
    focusedThreadId.value = 'waiting';
    paneOnScreen.value = true;
    expect(landOnEvent).not.toHaveBeenCalled();
  });

  it('opens a row of any other list as it was', () => {
    setSelectedOngoingGroup('in-flight');
    openListedThread('waiting');
    setDrawerGrouping('folders');
    setSelectedOngoingGroup('blocked');
    openListedThread('waiting');

    expect(focusThread).toHaveBeenCalledTimes(2);
    expect(landOnEvent).not.toHaveBeenCalled();
  });

  it('opens a search result as it was', () => {
    threadSearchQuery.value = 'which';
    openListedThread('waiting');

    expect(focusThread).toHaveBeenCalledOnce();
    expect(landOnEvent).not.toHaveBeenCalled();
  });
});

describe('openOngoingThread from Review', () => {
  beforeEach(resetAll);

  it.each([null, 'row:-20:reply-2', 'row:-20:reply-1', 'row:-20:unknown'])(
    'opens Review with saved position %s', saved => {
    const events = new Map<number, unknown>([
      [1, { type: 'MessageReceived', content: 'first', _eventId: 'start-1', created: '2026-08-06T10:00:00Z' }],
      [2, { type: 'ResponseGenerated', text: 'one', _eventId: 'reply-1', created: '2026-08-06T10:00:01Z' }],
      [3, { type: 'MessageReceived', content: 'write the report', _eventId: 'start-2', created: '2026-08-06T10:01:00Z' }],
      [4, { type: 'ThreadReadRequested', _eventId: 'asked', created: '2026-08-06T10:01:01Z' }],
      [5, { type: 'ResponseGenerated', text: 'the report', _eventId: 'reply-2', created: '2026-08-06T10:01:02Z' }],
    ]);
    threadMap.value = new Map([['asked', {
      meta: { id: 'asked', status: 'idle', readRequested: true, codingAgentChangeState: { kind: 'none' } },
      events,
      pendingUserMessages: [],
      eventsLoaded: true,
    } as unknown as ThreadState]]);

    if (saved) localStorage.setItem('lucidos-scroll-thread-asked', saved);
    openOngoingThread('asked', 'review');

    expect(focusThread).toHaveBeenCalledWith('asked');
    if (saved === 'row:-20:reply-2') expect(landOnEvent).not.toHaveBeenCalled();
    else expect(landOnEvent).toHaveBeenCalledWith('asked', 'start-2');
  });
});

describe('jumpableEventId', () => {
  it('rules out an event type that never anchors', () => {
    expect(jumpableEventId('bash-1', 'BackgroundBashCompleted')).toBeUndefined();
  });

  it('passes through any other event id', () => {
    expect(jumpableEventId('idle-1', 'CodingAgentIdled')).toBe('idle-1');
    expect(jumpableEventId(undefined, 'CodingAgentIdled')).toBeUndefined();
  });
});
