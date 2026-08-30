import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { signal } from '@preact/signals';
import type { Tap } from '@lucidos/sdk';
import type { Notification } from '../types';

const { isPageActive, isInViewport, markReadOptimistic } = vi.hoisted(() => ({
  isPageActive: vi.fn(() => true),
  isInViewport: vi.fn((_id: string) => false),
  markReadOptimistic: vi.fn((_id: string) => {}),
}));

vi.mock('../../utils/pageActive', () => ({ isPageActive }));
vi.mock('./notifications', () => ({ markReadOptimistic }));
vi.mock('../../utils/viewport', async () => {
  const { signal: sig } = await import('@preact/signals');
  const mobile = sig(false);
  return {
    isInViewport,
    viewportIsMobile: mobile,
    isMobile: () => mobile.value,
    isTouchDevice: () => false,
    isMobileOrTouch: () => mobile.value,
  };
});

import {
  activeMenuItem,
  focusedThreadId,
  mobileView,
  panelOverlay,
  settingsSubview,
  splitRatio,
  unreadNotifications,
} from '../store';
import {
  RENDER_WATCH_MS,
  SEEN_DWELL_MS,
  _resetSeenTargetWatchForTesting,
  contentPaneOnScreen,
  emptySeenWaits,
  installSeenTargetWatch,
  nextDwellDueIn,
  pruneSeenWaits,
  resampleSeenTargets,
  sampleSeen,
  threadPaneOnScreen,
  visitedKeys,
  type VisitLocation,
} from './notification-visit';
import { viewportIsMobile as mobileSignal } from '../../utils/viewport';

const DESKTOP: VisitLocation = {
  focusedThreadId: null,
  overlay: null,
  activeMenuItem: 'files',
  settingsSubview: 'main',
  mobile: false,
  mobileView: 'thread',
  splitRatio: 0.4,
};

function at(patch: Partial<VisitLocation>): VisitLocation {
  return { ...DESKTOP, ...patch };
}

function unread(id: string, tap: Tap): Notification {
  return {
    id,
    title: 't',
    message: 'm',
    created_at: '2026-08-30T00:00:00Z',
    read: false,
    tap,
  } as Notification;
}

const threadTap = (threadId: string, eventId?: string): Tap =>
  ({ kind: 'navigate', to: { target: 'thread', id: threadId, event_id: eventId } } as Tap);

// ---------------------------------------------------------------------------

describe('which panes are on screen', () => {
  it('shows both panes on a desktop split', () => {
    expect(threadPaneOnScreen(at({}))).toBe(true);
    expect(contentPaneOnScreen(at({}))).toBe(true);
  });

  it('hides the pane a desktop collapse zeroed', () => {
    expect(threadPaneOnScreen(at({ splitRatio: 0 }))).toBe(false);
    expect(contentPaneOnScreen(at({ splitRatio: 0 }))).toBe(true);
    expect(threadPaneOnScreen(at({ splitRatio: 1 }))).toBe(true);
    expect(contentPaneOnScreen(at({ splitRatio: 1 }))).toBe(false);
  });

  it('shows one pane at a time on mobile', () => {
    const onThread = at({ mobile: true, mobileView: 'thread' });
    expect(threadPaneOnScreen(onThread)).toBe(true);
    expect(contentPaneOnScreen(onThread)).toBe(false);
    const onContent = at({ mobile: true, mobileView: 'content' });
    expect(threadPaneOnScreen(onContent)).toBe(false);
    expect(contentPaneOnScreen(onContent)).toBe(true);
  });

  it('shows neither pane while the mobile drawer is up', () => {
    const onDrawer = at({ mobile: true, mobileView: 'threads' });
    expect(threadPaneOnScreen(onDrawer)).toBe(false);
    expect(contentPaneOnScreen(onDrawer)).toBe(false);
  });
});

describe('visitedKeys', () => {
  it('names the focused thread and the panel beside it', () => {
    expect(visitedKeys(at({ focusedThreadId: 't-1', activeMenuItem: 'changes' })))
      .toEqual(['thread:t-1', 'panel:changes']);
  });

  it('drops the thread the mobile reader has swiped away from', () => {
    expect(visitedKeys(at({ focusedThreadId: 't-1', mobile: true, mobileView: 'content' })))
      .toEqual(['panel:files']);
  });

  it('resolves settings down to its sub-section', () => {
    expect(visitedKeys(at({ activeMenuItem: 'settings', settingsSubview: 'backup' })))
      .toEqual(['settings:backup']);
  });

  it('names the open app by its id, unlike the content view key', () => {
    const overlay = { type: 'app-ui', app: { id: 'habit-tracker', name: 'H', description: '' } };
    expect(visitedKeys(at({ overlay: overlay as never }))).toEqual(['app:habit-tracker']);
  });

  it('names the previewed file and the edited trigger', () => {
    expect(visitedKeys(at({ overlay: { type: 'file-preview', path: 'artifacts/x.md' } })))
      .toEqual(['file:artifacts/x.md']);
    const form = { type: 'form', form: { type: 'trigger', triggerId: 'tr-1' } };
    expect(visitedKeys(at({ overlay: form as never }))).toEqual(['trigger:tr-1']);
  });

  it('names nothing for an overlay no tap can point at', () => {
    expect(visitedKeys(at({ overlay: { type: 'url-preview', url: 'https://example.com' } })))
      .toEqual([]);
  });
});

describe('the dwell', () => {
  it('reports nothing on the first sample, however long the list', () => {
    const waits = emptySeenWaits();
    expect(sampleSeen(waits, 1000, ['a', 'b'])).toEqual([]);
  });

  it('reports a target that held the whole dwell', () => {
    const waits = emptySeenWaits();
    sampleSeen(waits, 1000, ['a']);
    expect(sampleSeen(waits, 1000 + SEEN_DWELL_MS, ['a'])).toEqual(['a']);
  });

  it('reports nothing for a target that left early', () => {
    const waits = emptySeenWaits();
    sampleSeen(waits, 1000, ['a']);
    expect(sampleSeen(waits, 1500, [])).toEqual([]);
    // Coming back restarts the clock: time in the band must be continuous.
    sampleSeen(waits, 1600, ['a']);
    expect(sampleSeen(waits, 1600 + SEEN_DWELL_MS - 1, ['a'])).toEqual([]);
    expect(sampleSeen(waits, 1600 + SEEN_DWELL_MS, ['a'])).toEqual(['a']);
  });

  it('reports a target once, even if it keeps holding', () => {
    const waits = emptySeenWaits();
    sampleSeen(waits, 0, ['a']);
    expect(sampleSeen(waits, SEEN_DWELL_MS, ['a'])).toEqual(['a']);
    expect(sampleSeen(waits, SEEN_DWELL_MS * 5, ['a'])).toEqual([]);
  });

  it('forgets a notification the page no longer holds as unread', () => {
    const waits = emptySeenWaits();
    sampleSeen(waits, 0, ['a']);
    sampleSeen(waits, SEEN_DWELL_MS, ['a']);
    pruneSeenWaits(waits, new Set());
    expect(waits.done.size).toBe(0);
    expect(waits.since.size).toBe(0);
  });

  it('says when the next dwell is due', () => {
    const waits = emptySeenWaits();
    expect(nextDwellDueIn(waits, 0)).toBeNull();
    sampleSeen(waits, 1000, ['a']);
    expect(nextDwellDueIn(waits, 1400)).toBe(SEEN_DWELL_MS - 400);
    expect(nextDwellDueIn(waits, 9999)).toBe(0);
  });
});

// ---------------------------------------------------------------------------

interface ObserverFakes {
  /** One entry per constructed observer, holding its callback. */
  intersection: Array<() => void>;
  mutation: Array<() => void>;
  disconnected: number;
}

/** Stand in for the two DOM observers the watch arms.
 *
 *  The suite runs against the minimal stub page rather than jsdom, so neither
 *  exists. Faking them is also what makes the arming ORDER assertable: a real
 *  observer would only report through the browser. */
function installObserverFakes(): ObserverFakes {
  const fakes: ObserverFakes = { intersection: [], mutation: [], disconnected: 0 };
  const make = (into: Array<() => void>) => class {
    constructor(cb: () => void) { into.push(cb); }
    observe() { /* the fake reports through its captured callback */ }
    disconnect() { fakes.disconnected++; }
  };
  const g = globalThis as Record<string, unknown>;
  g.IntersectionObserver = make(fakes.intersection);
  g.MutationObserver = make(fakes.mutation);
  g.CSS = { escape: (s: string) => s };
  (globalThis.document as unknown as { body: unknown }).body = {};
  return fakes;
}

function stubQuerySelectorAll(rows: () => unknown[]): void {
  (globalThis.document as unknown as { querySelectorAll: unknown }).querySelectorAll =
    () => rows();
}

function removeObserverFakes(): void {
  const g = globalThis as Record<string, unknown>;
  delete g.IntersectionObserver;
  delete g.MutationObserver;
  delete g.CSS;
  (globalThis.document as unknown as { body: unknown }).body = undefined;
  (globalThis.document as unknown as { querySelectorAll: unknown }).querySelectorAll = () => [];
}

describe('the watch', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(0);
    isPageActive.mockReset().mockReturnValue(true);
    isInViewport.mockReset().mockReturnValue(false);
    markReadOptimistic.mockReset();
    (mobileSignal as ReturnType<typeof signal<boolean>>).value = false;
    focusedThreadId.value = null;
    panelOverlay.value = null;
    activeMenuItem.value = 'files';
    settingsSubview.value = 'main';
    splitRatio.value = 0.4;
    mobileView.value = 'thread';
    unreadNotifications.value = { status: 'not-loaded' };
    _resetSeenTargetWatchForTesting();
    installSeenTargetWatch();
  });

  afterEach(() => {
    unreadNotifications.value = { status: 'not-loaded' };
    _resetSeenTargetWatchForTesting();
    removeObserverFakes();
    vi.useRealTimers();
  });

  /** Two samples a dwell apart, which is what the live triggers do for free. */
  function dwell(): void {
    resampleSeenTargets();
    vi.setSystemTime(SEEN_DWELL_MS);
    resampleSeenTargets();
  }

  it('reads a notification whose event card is in the band', () => {
    focusedThreadId.value = 't-1';
    isInViewport.mockReturnValue(true);
    unreadNotifications.value = { status: 'loaded', data: [unread('n-1', threadTap('t-1', 'e-1'))] };
    dwell();
    expect(markReadOptimistic).toHaveBeenCalledWith('n-1');
  });

  it('leaves a notification whose card is scrolled away', () => {
    // This is §4 Row 2. Being on the thread is not being at the event.
    focusedThreadId.value = 't-1';
    isInViewport.mockReturnValue(false);
    unreadNotifications.value = { status: 'loaded', data: [unread('n-1', threadTap('t-1', 'e-1'))] };
    dwell();
    expect(markReadOptimistic).not.toHaveBeenCalled();
  });

  it('leaves a notification whose card is in another thread', () => {
    focusedThreadId.value = 't-2';
    isInViewport.mockReturnValue(true);
    unreadNotifications.value = { status: 'loaded', data: [unread('n-1', threadTap('t-1', 'e-1'))] };
    dwell();
    expect(markReadOptimistic).not.toHaveBeenCalled();
  });

  it('leaves a notification whose card sits in a collapsed pane', () => {
    focusedThreadId.value = 't-1';
    splitRatio.value = 0;
    isInViewport.mockReturnValue(true);
    unreadNotifications.value = { status: 'loaded', data: [unread('n-1', threadTap('t-1', 'e-1'))] };
    dwell();
    expect(markReadOptimistic).not.toHaveBeenCalled();
  });

  it('reads a card-less notification once its place is on screen', () => {
    activeMenuItem.value = 'settings';
    settingsSubview.value = 'backup';
    const tap = { kind: 'navigate', to: { target: 'settings', settings_view: 'backup' } } as Tap;
    unreadNotifications.value = { status: 'loaded', data: [unread('n-1', tap)] };
    dwell();
    expect(markReadOptimistic).toHaveBeenCalledWith('n-1');
  });

  it('never reads a modal notification, whatever is on screen', () => {
    focusedThreadId.value = 't-1';
    isInViewport.mockReturnValue(true);
    const row = unread('n-1', { kind: 'modal' });
    // Provenance only. Its thread being read must not clear it.
    (row as { thread_id?: string }).thread_id = 't-1';
    unreadNotifications.value = { status: 'loaded', data: [row] };
    dwell();
    expect(markReadOptimistic).not.toHaveBeenCalled();
  });

  it('reads nothing while the page is hidden', () => {
    focusedThreadId.value = 't-1';
    isInViewport.mockReturnValue(true);
    isPageActive.mockReturnValue(false);
    unreadNotifications.value = { status: 'loaded', data: [unread('n-1', threadTap('t-1', 'e-1'))] };
    dwell();
    expect(markReadOptimistic).not.toHaveBeenCalled();
  });

  it('starts the dwell again after a background, never resuming it', () => {
    focusedThreadId.value = 't-1';
    isInViewport.mockReturnValue(true);
    unreadNotifications.value = { status: 'loaded', data: [unread('n-1', threadTap('t-1', 'e-1'))] };
    resampleSeenTargets();
    isPageActive.mockReturnValue(false);
    vi.setSystemTime(500);
    resampleSeenTargets();
    isPageActive.mockReturnValue(true);
    vi.setSystemTime(SEEN_DWELL_MS + 1);
    resampleSeenTargets();
    expect(markReadOptimistic).not.toHaveBeenCalled();
    vi.setSystemTime(SEEN_DWELL_MS * 2 + 2);
    resampleSeenTargets();
    expect(markReadOptimistic).toHaveBeenCalledWith('n-1');
  });

  it('honours a place already on screen when the unread set lands late', () => {
    // The cold open: the thread is restored and rendered while the startup
    // fetch is still in flight, so the first samples have nothing to watch.
    focusedThreadId.value = 't-1';
    isInViewport.mockReturnValue(true);
    resampleSeenTargets();
    vi.setSystemTime(5000);
    unreadNotifications.value = { status: 'loaded', data: [unread('n-1', threadTap('t-1', 'e-1'))] };
    resampleSeenTargets();
    vi.setSystemTime(5000 + SEEN_DWELL_MS);
    resampleSeenTargets();
    expect(markReadOptimistic).toHaveBeenCalledWith('n-1');
  });

  it('reads each notification once, however many samples land', () => {
    focusedThreadId.value = 't-1';
    isInViewport.mockReturnValue(true);
    unreadNotifications.value = { status: 'loaded', data: [unread('n-1', threadTap('t-1', 'e-1'))] };
    dwell();
    for (let i = 2; i < 8; i++) {
      vi.setSystemTime(SEEN_DWELL_MS * i);
      resampleSeenTargets();
    }
    expect(markReadOptimistic).toHaveBeenCalledTimes(1);
  });

  /** Long enough to cross a frame, so the coalesced sample actually runs. */
  const A_FRAME = 50;

  it('samples on its own when the location moves, and again when the dwell is up', () => {
    // The whole chain with nothing driven by hand: the location effect, the
    // coalescer, and the timer the first sample schedules for itself.
    isInViewport.mockReturnValue(true);
    unreadNotifications.value = { status: 'loaded', data: [unread('n-1', threadTap('t-1', 'e-1'))] };
    focusedThreadId.value = 't-1';
    vi.advanceTimersByTime(A_FRAME);
    expect(markReadOptimistic).not.toHaveBeenCalled();
    vi.advanceTimersByTime(SEEN_DWELL_MS);
    expect(markReadOptimistic).toHaveBeenCalledWith('n-1');
  });

  it('wakes when the card finally renders, with nothing else firing', () => {
    // The bug this pins: a sample can run before Preact commits the transcript,
    // so the viewport observer arms on no element. Nothing else fires after a
    // render, and the rule slept with its card on screen.
    const observers = installObserverFakes();
    let cards: unknown[] = [];
    stubQuerySelectorAll(() => cards);

    focusedThreadId.value = 't-1';
    unreadNotifications.value = { status: 'loaded', data: [unread('n-1', threadTap('t-1', 'e-1'))] };
    resampleSeenTargets();

    // The card is not in the DOM yet, so the render watch is what stands guard.
    expect(observers.intersection).toHaveLength(0);
    expect(observers.mutation).toHaveLength(1);

    // The transcript commits. Only the render watch can report it.
    cards = [{}];
    isInViewport.mockReturnValue(true);
    observers.mutation[0]();
    vi.advanceTimersByTime(A_FRAME);
    vi.advanceTimersByTime(SEEN_DWELL_MS);
    expect(markReadOptimistic).toHaveBeenCalledWith('n-1');
  });

  it('gives the render watch up after its deadline, so it cannot outlive the gap', () => {
    // A windowed transcript can leave an older card permanently absent, and a
    // whole-body observer must not run for the rest of that visit.
    const observers = installObserverFakes();
    stubQuerySelectorAll(() => []);

    focusedThreadId.value = 't-1';
    unreadNotifications.value = { status: 'loaded', data: [unread('n-1', threadTap('t-1', 'e-1'))] };
    resampleSeenTargets();
    expect(observers.mutation).toHaveLength(1);
    expect(observers.disconnected).toBe(0);

    vi.advanceTimersByTime(RENDER_WATCH_MS + 1);
    expect(observers.disconnected).toBe(1);
  });

  it('a blur drops the dwell in flight, and the paired focus starts a new one', () => {
    // A window can lose OS focus without ever hiding, which `isPageActive()`
    // reads as inactive on desktop. Nothing in the page-visit pairing fires
    // there, so this rule listens for the raw pair.
    //
    // `isPageActive` is deliberately left TRUE throughout, so what is measured
    // is the blur handler's own effect rather than the gate's. A wait that
    // survived the blur would complete on the advance below.
    focusedThreadId.value = 't-1';
    isInViewport.mockReturnValue(true);
    unreadNotifications.value = { status: 'loaded', data: [unread('n-1', threadTap('t-1', 'e-1'))] };
    resampleSeenTargets();

    window.dispatchEvent(new Event('blur'));
    vi.advanceTimersByTime(SEEN_DWELL_MS * 2);
    expect(markReadOptimistic).not.toHaveBeenCalled();

    window.dispatchEvent(new Event('focus'));
    vi.advanceTimersByTime(A_FRAME);
    vi.advanceTimersByTime(SEEN_DWELL_MS);
    expect(markReadOptimistic).toHaveBeenCalledWith('n-1');
  });

  it('samples on its own when the reader scrolls', () => {
    // The card enters the band with no navigation at all, which is the case a
    // location-only rule would miss.
    focusedThreadId.value = 't-1';
    unreadNotifications.value = { status: 'loaded', data: [unread('n-1', threadTap('t-1', 'e-1'))] };
    vi.advanceTimersByTime(SEEN_DWELL_MS * 2);
    expect(markReadOptimistic).not.toHaveBeenCalled();
    isInViewport.mockReturnValue(true);
    document.dispatchEvent({ type: 'scroll' } as Event);
    vi.advanceTimersByTime(A_FRAME);
    vi.advanceTimersByTime(SEEN_DWELL_MS);
    expect(markReadOptimistic).toHaveBeenCalledWith('n-1');
  });

  it('reads nothing while stepping through threads faster than the dwell', () => {
    // Drawer browsing with the arrow keys. Each thread's card is in the band on
    // arrival, and none of them is read.
    isInViewport.mockReturnValue(true);
    unreadNotifications.value = {
      status: 'loaded',
      data: [
        unread('n-1', threadTap('t-1', 'e-1')),
        unread('n-2', threadTap('t-2', 'e-2')),
        unread('n-3', threadTap('t-3', 'e-3')),
      ],
    };
    for (const [i, id] of ['t-1', 't-2', 't-3'].entries()) {
      focusedThreadId.value = id;
      vi.setSystemTime(i * 200);
      resampleSeenTargets();
    }
    expect(markReadOptimistic).not.toHaveBeenCalled();
    // Resting on the last one reads only that one.
    vi.setSystemTime(400 + SEEN_DWELL_MS);
    resampleSeenTargets();
    expect(markReadOptimistic).toHaveBeenCalledTimes(1);
    expect(markReadOptimistic).toHaveBeenCalledWith('n-3');
  });
});
