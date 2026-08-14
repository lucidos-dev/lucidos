import { describe, it, expect, beforeEach, afterEach, vi, type Mock } from 'vitest';

import { scrollToEventAndPulse, scrollToChangeAndPulse, hasPendingEventScroll, clearPendingEventScroll, followingLiveEdge, isFollowScroll, makeScrollObservers, scrollToBottom, scrollToTop, isEventInViewport, isHeaderPinnedForScroll, setActiveScrollElement, setFollowLiveEdge, setThreadLive, stopFollowingBottom, resumeFollowingBottom } from '../scrollState';
import { hasNavFocus, clearNavFocus, NAV_FOCUS_FADE_MS, NAV_FOCUS_HOLD_MS, NAV_FOCUS_RAMP_MS } from '../../shared/focusMarker';

/** The deep-link now scrolls via the shared animateScroll engine (a rAF tween
 *  writing scrollTop on the active container), NOT native scrollIntoView. Tests
 *  that assert the landing register this fake container and advance fake timers.
 *  Its getBoundingClientRect top is 0, so an element whose rect top is
 *  `absTop − container.scrollTop` (see makeTargetEl) yields a STABLE target of
 *  `absTop`, and the tween lands scrollTop exactly there. */
function makeContainer(scrollTop = 0) {
  return {
    parentElement: null,
    scrollTop,
    scrollHeight: 10000,
    clientHeight: 800,
    getBoundingClientRect: () => ({ width: 400, height: 800, top: 0, bottom: 800, left: 0, right: 400 }),
    addEventListener: () => {},
    removeEventListener: () => {},
  } as any;
}

type MOCallback = (records: MutationRecord[], observer: MutationObserver) => void;
const moObservations: Array<{ target: any; options: any }> = [];
let lastMoCallback: MOCallback | null = null;

class FakeMutationObserver {
  constructor(cb: MOCallback) { lastMoCallback = cb; }
  observe(target: any, options: any) { moObservations.push({ target, options }); }
  disconnect() {}
  takeRecords() { return []; }
}

const fakeBody = { tagName: 'BODY' };

function installFakeDom(opts: { threadContents?: any[]; dataEventMatches?: any[] } = {}) {
  const orig = {
    documentBody: (globalThis.document as any).body,
    documentQSA: (globalThis.document as any).querySelectorAll,
    MutationObserver: (globalThis as any).MutationObserver,
    CSS: (globalThis as any).CSS,
    getComputedStyle: (globalThis as any).getComputedStyle,
  };

  (globalThis.document as any).body = fakeBody;
  (globalThis.document as any).querySelectorAll = (sel: string) => {
    if (sel === '.thread-content') return opts.threadContents ?? [];
    // Both deep-link selectors resolve from the same fixture list so a single
    // installFakeDom call serves event-id and change-id scrolls alike.
    if (sel.startsWith('[data-event-id') || sel.startsWith('[data-change-id')) return opts.dataEventMatches ?? [];
    return [];
  };
  (globalThis as any).MutationObserver = FakeMutationObserver;
  (globalThis as any).CSS = { escape: (s: string) => s };
  // smoothScrollToElement reads the target's scroll-margin-top; the fake targets
  // aren't real Elements, so stub a zero margin (real getComputedStyle throws on
  // a plain object).
  (globalThis as any).getComputedStyle = () => ({ scrollMarginTop: '0px' });

  return () => {
    (globalThis.document as any).body = orig.documentBody;
    (globalThis.document as any).querySelectorAll = orig.documentQSA;
    (globalThis as any).MutationObserver = orig.MutationObserver;
    (globalThis as any).CSS = orig.CSS;
    (globalThis as any).getComputedStyle = orig.getComputedStyle;
  };
}

beforeEach(() => {
  moObservations.length = 0;
  lastMoCallback = null;
  // Reset module-level deep-link claim state so a held claim (a sync resolve now
  // holds it across the smooth-scroll settle; an unresolved async path holds it
  // until its deadline) can't leak into the next test.
  clearPendingEventScroll();
});

describe('scrollToEventAndPulse — MutationObserver setup', () => {
  let restore: (() => void) | null = null;
  afterEach(() => { restore?.(); restore = null; });

  it('observes document.body, not the loading-state .thread-content (which gets detached on ThreadView loading→loaded swap)', () => {
    // ThreadView's loading branch and loaded branch are structurally different
    // children of .thread-view (1 vs 2), so Preact's positional diff cannot
    // preserve the loading-branch .thread-content across the swap. An observer
    // scoped to it would strand. iOS PWA cold-start hits this every time.
    const loadingThreadContent = { tagName: 'DIV', className: 'thread-content' };
    restore = installFakeDom({ threadContents: [loadingThreadContent] });

    scrollToEventAndPulse('e-7');

    expect(moObservations).toHaveLength(1);
    expect(moObservations[0].target).toBe(fakeBody);
    expect(moObservations[0].options).toMatchObject({ childList: true, subtree: true });
  });

  it('observes document.body when no .thread-content exists yet', () => {
    restore = installFakeDom({});
    scrollToEventAndPulse('e-7');
    expect(moObservations).toHaveLength(1);
    expect(moObservations[0].target).toBe(fakeBody);
  });

  it('is a no-op when called with an empty event id', () => {
    restore = installFakeDom({});
    scrollToEventAndPulse('');
    expect(moObservations).toHaveLength(0);
  });
});

describe('scrollToEventAndPulse — MutationObserver callback filter', () => {
  let restore: (() => void) | null = null;
  afterEach(() => { restore?.(); restore = null; });

  function makeNode(opts: {
    nodeType?: number;
    matchesSelector?: boolean;
    querySelectorResult?: any;
  }): Node & Element {
    let qsCalls = 0;
    let matchCalls = 0;
    const node: any = {
      nodeType: opts.nodeType ?? 1,
      matches: (_sel: string) => { matchCalls++; return !!opts.matchesSelector; },
      querySelector: (_sel: string) => { qsCalls++; return opts.querySelectorResult ?? null; },
      get _matchCalls() { return matchCalls; },
      get _qsCalls() { return qsCalls; },
    };
    return node;
  }

  function fireMutation(addedNodes: Node[]) {
    const records = [{ addedNodes, type: 'childList' } as unknown as MutationRecord];
    lastMoCallback?.(records, {} as MutationObserver);
  }

  it('does NOT re-query document when added nodes do not contain the target (streaming-token wakeup case)', () => {
    restore = installFakeDom({});
    let docQsaCalls = 0;
    const origQsa = (globalThis.document as any).querySelectorAll;
    (globalThis.document as any).querySelectorAll = (sel: string) => {
      docQsaCalls++;
      return origQsa(sel);
    };

    scrollToEventAndPulse('e-7');
    const docQsaCallsAfterSetup = docQsaCalls;

    const streamingTextNode = makeNode({ nodeType: 3 });
    const unrelatedElement = makeNode({ matchesSelector: false, querySelectorResult: null });
    fireMutation([streamingTextNode, unrelatedElement]);

    // No call into document.querySelectorAll past the initial setup.
    expect(docQsaCalls).toBe(docQsaCallsAfterSetup);
  });

  it('re-queries when an added node IS the target', () => {
    restore = installFakeDom({});
    scrollToEventAndPulse('e-7');

    let queriedSelector: string | null = null;
    (globalThis.document as any).querySelectorAll = (sel: string) => {
      queriedSelector = sel;
      return [];
    };

    fireMutation([makeNode({ matchesSelector: true })]);

    expect(queriedSelector).toBe('[data-event-id="e-7"]');
  });

  it('re-queries when an added subtree CONTAINS the target', () => {
    restore = installFakeDom({});
    scrollToEventAndPulse('e-7');

    let queriedSelector: string | null = null;
    (globalThis.document as any).querySelectorAll = (sel: string) => {
      queriedSelector = sel;
      return [];
    };

    fireMutation([makeNode({ matchesSelector: false, querySelectorResult: {} })]);

    expect(queriedSelector).toBe('[data-event-id="e-7"]');
  });
});

describe('scrollToEventAndPulse — deep-link scroll suppression', () => {
  // Regression: tapping a notification that targets a specific event in a
  // thread that is NOT already focused used to land at the thread bottom
  // instead of the event. Focusing an unfocused thread lazily loads its
  // events, and the scroll-to-bottom that fired on the eventsLoaded false→true
  // transition overrode the deep-link scroll the moment they rendered. The fix
  // was a "pending event scroll" claim those callers consult. Those particular
  // callers are gone, but the claim is not: `useScrollMemory`'s restore (and
  // its open-at-the-top reset) wake on that same render and would win the same
  // way, so the landing is still held until the deep-link settles.
  let restore: (() => void) | null = null;
  let container: any;

  beforeEach(() => {
    vi.useFakeTimers();
    container = makeContainer();
    setActiveScrollElement(container);
  });
  afterEach(() => {
    vi.clearAllTimers();
    vi.useRealTimers();
    setActiveScrollElement(null);
    restore?.();
    restore = null;
  });

  /** A DOM element that passes isElementVisible (non-zero rect, no clipping
   *  ancestor). Its rect top is `absTop − container.scrollTop`, so the animateScroll
   *  tween lands container.scrollTop exactly at `absTop`. */
  function makeVisibleEl(absTop = 3000) {
    const el: any = {
      parentElement: null,
      getBoundingClientRect: () => ({ width: 200, height: 200, top: absTop - container.scrollTop, bottom: absTop - container.scrollTop + 200, left: 0, right: 200 }),
      classList: { add: () => {}, remove: () => {} },
    };
    return el;
  }

  function makeTargetNode(): Node & Element {
    return { nodeType: 1, matches: () => true, querySelector: () => null } as any;
  }

  function fireMutation(addedNodes: Node[]) {
    const records = [{ addedNodes, type: 'childList' } as unknown as MutationRecord];
    lastMoCallback?.(records, {} as MutationObserver);
  }

  it('resolves synchronously when the event is already in the DOM (focused-thread path) — HOLDS the claim across the smooth-scroll settle, then releases', () => {
    const visibleEl = makeVisibleEl();
    restore = installFakeDom({ dataEventMatches: [visibleEl] });

    scrollToEventAndPulse('e-7');

    // The claim is NOT released synchronously — the deep-link tween is still
    // settling, and a competing scroll (a saved-position restore waking on the
    // same render, a panel close) in that window would override the landing.
    // It's held until scrollend / the fallback timer.
    expect(hasPendingEventScroll()).toBe(true);
    // Resolved before ever observing — no need to wait for lazily-loaded events.
    expect(moObservations).toHaveLength(0);

    // The rAF tween runs and lands scrollTop on the event's position. 800ms is
    // past the tween's duration (≤ SCROLL_MAX_MS) but before the claim fallback.
    vi.advanceTimersByTime(800);
    expect(container.scrollTop).toBe(3000);
    expect(hasPendingEventScroll()).toBe(true); // claim still held

    // Fallback timer fires (no scrollend in jsdom) → claim released.
    vi.advanceTimersByTime(300); // total 1100, past SCROLL_SETTLE_FALLBACK_MS (1000)
    expect(hasPendingEventScroll()).toBe(false);
  });

  it('stays pending while the lazily-loaded event has not rendered (unfocused-thread path), lands on resolve, and HOLDS the claim until the deadline', () => {
    restore = installFakeDom({}); // target not in the DOM yet

    scrollToEventAndPulse('e-7');
    // This is the flag `useScrollMemory` consults to defer its restore (and its
    // open-at-the-top reset) so neither can override the upcoming deep-link
    // scroll. It used to guard a fleet of auto-scroll-to-bottom callers too,
    // which no longer exist.
    expect(hasPendingEventScroll()).toBe(true);
    expect(moObservations).toHaveLength(1);

    // Events render: the target card appears and is visible.
    const visibleEl = makeVisibleEl();
    (globalThis.document as any).querySelectorAll = (sel: string) =>
      sel.startsWith('[data-event-id') ? [visibleEl] : [];
    fireMutation([makeTargetNode()]);

    vi.advanceTimersByTime(800); // run the tween → lands on the event
    expect(container.scrollTop).toBe(3000);
    // The claim is HELD past the scroll: the same render that revealed the
    // event is what wakes the saved-scroll restore observers, which the claim
    // must keep suppressed. It releases only at the deadline.
    expect(hasPendingEventScroll()).toBe(true);

    vi.advanceTimersByTime(5000); // past EVENT_RESOLVE_DEADLINE_MS (4000)
    expect(hasPendingEventScroll()).toBe(false);
  });

  it('clearPendingEventScroll() cancels an in-flight claim (a plain focus superseding a deep-link)', () => {
    // A plain thread focus cancels a prior deep-link's claim so its suppression
    // can't leak onto the newly-focused thread's load.
    restore = installFakeDom({}); // target never appears → claim stays pending
    scrollToEventAndPulse('e-7');
    expect(hasPendingEventScroll()).toBe(true);

    clearPendingEventScroll();
    expect(hasPendingEventScroll()).toBe(false);
  });

  it('scrollToBottom() supersedes an in-flight claim (the down chevron, tapped mid-resolve)', () => {
    // The chevron is the reader saying "take me to the live edge", which
    // overrides a landing they are no longer waiting for. It is now the ONLY
    // caller of scrollToBottom, so there is no automatic variant that has to
    // defer to the claim instead.
    restore = installFakeDom({}); // target never appears → claim stays pending
    scrollToEventAndPulse('e-7');
    expect(hasPendingEventScroll()).toBe(true);

    scrollToBottom();
    expect(hasPendingEventScroll()).toBe(false);
  });

  it('clears the pending flag when the deadline passes without the event', () => {
    restore = installFakeDom({}); // target never appears

    scrollToEventAndPulse('e-7');
    expect(hasPendingEventScroll()).toBe(true);

    vi.advanceTimersByTime(5000); // past EVENT_RESOLVE_DEADLINE_MS (4000)

    expect(hasPendingEventScroll()).toBe(false);
  });
});

describe('a deep-link landing retires a standing follow only when it lands OFF the live edge', () => {
  /** Going to a link is the reader asking to be at ONE place, so the ride ends
   *  there. Unless the place IS the live edge: the two asks agree, and there is
   *  nothing to end. `stepThreadTurn` already asks that of its own landing, and
   *  the deep link was the one navigation that never did.
   *
   *  The disarm in `onScroll` cannot answer it. That wants the reader off the
   *  live edge AND away from the follow's last write, and a landing at the edge
   *  is neither.
   *
   *  It is measured on the LANDING, not on the pixels moved. A link carrying a
   *  scrolled-up rider TO the live edge moves them a long way. It keeps the
   *  ride, having put them where the ride wanted them.
   *
   *  Retiring is not gated on the thread being LIVE, unlike the scroll disarm,
   *  the up chevron and turn stepping. A link names one event and expects to
   *  still be on it later, so the ask survives the thread waking. That half is
   *  the block below. What ARMS the same follow is pinned in
   *  `scroll-follow-the-live-edge.test.ts`. */
  let restore: (() => void) | null = null;
  let container: any;

  beforeEach(() => {
    vi.useFakeTimers();
    container = makeContainer();
    setActiveScrollElement(container);
    // The premise of every test in this block. `setThreadLive` is a module
    // global, so the afterEach un-says it rather than leaving a live thread for
    // the next block.
    setThreadLive(true);
  });
  afterEach(() => {
    vi.clearAllTimers();
    vi.useRealTimers();
    stopFollowingBottom();
    setThreadLive(false);
    setActiveScrollElement(null);
    restore?.();
    restore = null;
  });

  /** As in the block above: a rect top of `absTop − container.scrollTop` gives
   *  the tween a stable target of `absTop`. */
  function makeVisibleEl(absTop: number) {
    return {
      parentElement: null,
      getBoundingClientRect: () => ({
        width: 200, height: 200, top: absTop - container.scrollTop, bottom: absTop - container.scrollTop + 200, left: 0, right: 200,
      }),
      classList: { add: () => {}, remove: () => {} },
    } as any;
  }

  it('keeps the ride when the link lands ON the live edge', () => {
    // The reported case. The target is the thread's newest turn, so the landing
    // rests exactly where the ride was already holding the reader. Nothing
    // moved, so there is nothing for the ride to be inconsistent with.
    restore = installFakeDom({ dataEventMatches: [makeVisibleEl(9200)] });
    const { onResize } = makeScrollObservers(container);

    setFollowLiveEdge(true);          // the reader arms the follow
    vi.advanceTimersByTime(1500);     // its glide settles on the live edge
    // 9200, the MAX offset, and the same place the link below resolves to,
    // which is the whole point of this test. (It read 10000 while the arming
    // was the chevron's, because `scrollToBottom` writes the raw `scrollHeight`
    // and leans on the browser's clamp, which this fake container has not got.)
    expect(container.scrollTop).toBe(9200);

    scrollToEventAndPulse('e-7');     // and then taps a notification
    vi.advanceTimersByTime(1500);     // the landing settles
    expect(container.scrollTop).toBe(9200);
    expect(followingLiveEdge.value).toBe(true);

    container.scrollHeight = 20000;   // they answer, and the agent replies

    onResize();

    expect(container.scrollTop).toBe(19200);
    // Recorded as the live edge, not as the offset the landing produced. So
    // coming back to the thread resumes the ride instead of parking them.
    expect(isFollowScroll(container)).toBe(true);
  });

  it('ends the ride when the link lands ABOVE the live edge', () => {
    // The other side of the same measurement. The link names a place the ride
    // disagrees with, so the ride ends where it puts them. Left armed, the next
    // token would carry them off the very event they asked to see.
    restore = installFakeDom({ dataEventMatches: [makeVisibleEl(3000)] });
    const { onResize } = makeScrollObservers(container);

    setFollowLiveEdge(true);
    vi.advanceTimersByTime(1500);
    scrollToEventAndPulse('e-7');
    vi.advanceTimersByTime(1500);
    expect(container.scrollTop).toBe(3000);
    expect(followingLiveEdge.value).toBe(false);

    container.scrollHeight = 20000;
    onResize();

    expect(container.scrollTop).toBe(3000);
  });

  it('measures the landing, so a target just short of the edge still ends it', () => {
    // The retirement and the scroll read ONE target (`landingTargetOf`), which
    // is what stops them disagreeing about where the reader is going to end up.
    // 9100 is inside the last viewport and well outside the edge's 2px slack,
    // so it is a place like any other.
    restore = installFakeDom({ dataEventMatches: [makeVisibleEl(9100)] });

    setFollowLiveEdge(true);
    vi.advanceTimersByTime(1500);
    scrollToEventAndPulse('e-7');
    vi.advanceTimersByTime(1500);

    expect(container.scrollTop).toBe(9100);
    expect(followingLiveEdge.value).toBe(false);
  });

  it('supersedes a tween that was taking the reader somewhere else', () => {
    // Keeping the ride must not cost the link its ownership of the viewport.
    // The reader is at the edge, so the ride writes nothing. An up-chevron
    // glide tapped a frame earlier would otherwise survive it. It would then
    // carry them to the top, the link's marker left on a turn at the bottom.
    restore = installFakeDom({ dataEventMatches: [makeVisibleEl(9200)] });
    makeScrollObservers(container);

    setFollowLiveEdge(true);
    vi.advanceTimersByTime(1500);
    setThreadLive(false);         // idle, so the chevron leaves the ride armed
    scrollToTop();                // and its glide is in flight, having moved nobody yet
    expect(container.scrollTop).toBe(9200);

    scrollToEventAndPulse('e-7');
    vi.advanceTimersByTime(1500);

    expect(container.scrollTop).toBe(9200);
    expect(followingLiveEdge.value).toBe(true);
  });

  it('keeps the ride when the link CARRIES a scrolled-up rider to the live edge', () => {
    // Pixels moved is the wrong question. The platform left this armed reader
    // 6200px up, with no gesture, so the ride survived. The link takes them all
    // the way back down, which is where the ride wanted them.
    restore = installFakeDom({ dataEventMatches: [makeVisibleEl(9200)] });
    const { onResize } = makeScrollObservers(container);

    setFollowLiveEdge(true);
    vi.advanceTimersByTime(1500);
    container.scrollTop = 3000;

    scrollToEventAndPulse('e-7');
    vi.advanceTimersByTime(1500);

    expect(container.scrollTop).toBe(9200);
    expect(followingLiveEdge.value).toBe(true);
    // The ride's OWN motion took them there, so its frames are held writes and
    // the position still records as the live edge.
    expect(isFollowScroll(container)).toBe(true);

    container.scrollHeight = 20000;
    onResize();

    expect(container.scrollTop).toBe(19200);
  });

  it('answers to WHERE a superseded call landed, that being where the reader is', () => {
    // A superseded call still lands, and still acts on the ride. So the place it
    // rested is what a later resume has to answer to. Let the newer claim speak
    // for it and the resume glides the reader off the event they are looking at.
    const matches: any[] = [];
    restore = installFakeDom({ dataEventMatches: matches });
    makeScrollObservers(container);

    setFollowLiveEdge(true);
    vi.advanceTimersByTime(1500);
    scrollToEventAndPulse('e-old');   // still resolving, so its observer is armed

    matches.push(makeVisibleEl(9200));
    scrollToEventAndPulse('e-new');   // a second tap, landing ON the edge
    vi.advanceTimersByTime(50);
    expect(followingLiveEdge.value).toBe(true);

    // The older call finds its target late, well above the edge.
    matches.length = 0;
    matches.push(makeVisibleEl(3000));
    lastMoCallback?.(
      [{ addedNodes: [{ nodeType: 1, matches: () => true, querySelector: () => null }] }] as any,
      {} as any,
    );
    // Long enough for its glide, short of the newer claim's own settle release.
    vi.advanceTimersByTime(800);
    expect(container.scrollTop).toBe(3000);
    expect(followingLiveEdge.value).toBe(false);

    // The resume must decline: the reader is on the older call's event.
    resumeFollowingBottom(container, 'in-place');

    expect(followingLiveEdge.value).toBe(false);
    expect(container.scrollTop).toBe(3000);
  });

  it('honours a superseded landing while the NEWER link is still resolving', () => {
    // The newer claim has no resolve to report yet. Asking whether IT landed
    // therefore says nothing about the reader, who is sitting where the older
    // call put them. Re-arming there hands the ride back over a landing that
    // ended it. A newer link that then turns out dead leaves them following
    // from the older one's event.
    const matches: any[] = [];
    restore = installFakeDom({ dataEventMatches: matches });
    makeScrollObservers(container);

    setFollowLiveEdge(true);
    vi.advanceTimersByTime(1500);
    scrollToEventAndPulse('e-old');
    const oldCallback = lastMoCallback;   // captured before the newer link takes the slot
    scrollToEventAndPulse('e-new');       // still resolving, and it may never land

    matches.push(makeVisibleEl(3000));
    oldCallback?.(
      [{ addedNodes: [{ nodeType: 1, matches: () => true, querySelector: () => null }] }] as any,
      {} as any,
    );
    vi.advanceTimersByTime(800);
    expect(container.scrollTop).toBe(3000);
    expect(followingLiveEdge.value).toBe(false);

    resumeFollowingBottom(container, 'in-place');

    expect(followingLiveEdge.value).toBe(false);
    expect(container.scrollTop).toBe(3000);
  });

  it('leaves a ride alone when the link never lands', () => {
    // Retiring belongs to the LANDING, not to the tap. A dead link moves the
    // reader nowhere, so there is nothing for the ride to be inconsistent with,
    // and taking it away would be the app dropping a request over a navigation
    // that never happened.
    restore = installFakeDom({}); // the target never renders
    const { onResize } = makeScrollObservers(container);

    setFollowLiveEdge(true);
    vi.advanceTimersByTime(1500);
    scrollToEventAndPulse('e-7');
    vi.advanceTimersByTime(5000); // past EVENT_RESOLVE_DEADLINE_MS

    container.scrollHeight = 20000;
    onResize();

    expect(container.scrollTop).toBe(19200);
  });

  it('does not write the live edge over a link that is still resolving', () => {
    // The follow puts an armed reader back on the live edge when the PLATFORM
    // scrolls them off it (`keepTheLiveEdge`, pinned in
    // `scroll-follow-the-live-edge.test.ts`). A deep link owns the position for
    // its whole resolve window, and for most of that window there is no tween to
    // stand down for: the thread is still loading and the target has not
    // rendered. So the claim is the guard, exactly as it is for the box-change
    // branch, and without it the reader would be hauled to the bottom by any
    // scroll arriving while their tap was still being answered.
    restore = installFakeDom({}); // the target has not rendered yet
    const { onScroll } = makeScrollObservers(container);

    setFollowLiveEdge(true);
    vi.advanceTimersByTime(1500);
    onScroll();                   // the glide's own event, recording them on the edge
    expect(container.scrollTop).toBe(9200);

    scrollToEventAndPulse('e-7'); // the tap, still resolving
    expect(hasPendingEventScroll()).toBe(true);

    container.scrollTop = 0;      // and the platform moves the container meanwhile
    onScroll();

    expect(container.scrollTop).toBe(0);
  });
});

describe('a deep-link landing ends the ride on an IDLE thread too', () => {
  /** The other half of the block above, and the one place the follow does NOT
   *  follow the idle rule the scroll disarm, the up chevron and turn stepping
   *  share.
   *
   *  Those three describe a moment: where the reader happens to be looking on a
   *  thread that is doing nothing. Keeping the ride costs them nothing, because
   *  nothing is running to carry them anywhere. A LINK is different in kind: it
   *  names ONE event and expects to still be on it later, so the ask has to
   *  survive the thread waking.
   *
   *  It did not. `waiting_for_user_answer` is quiescent (`isRenderedThreadIdle`),
   *  so a thread parked on a question card reads as IDLE here, and a "needs your
   *  answer" notification points at exactly such a thread. The reader tapped it,
   *  landed on the question, kept the lit toggle, answered, and `honourWake`
   *  wrote them to the live edge the instant the agent picked the answer up. Off
   *  the event the notification existed to show them, one beat after they got
   *  there. Reported 2026-08-12; the tests below used to assert that outcome.
   *
   *  A DEAD link still keeps the ride (the block above): retiring belongs to the
   *  landing, not to the tap. */
  let restore: (() => void) | null = null;
  let container: any;

  beforeEach(() => {
    vi.useFakeTimers();
    container = makeContainer();
    setActiveScrollElement(container);
    // Deliberately never `setThreadLive(true)`: an idle thread is the premise.
  });
  afterEach(() => {
    vi.clearAllTimers();
    vi.useRealTimers();
    setFollowLiveEdge(false); // un-press, so the *follow seed* does not leak
    setThreadLive(false);
    setActiveScrollElement(null);
    restore?.();
    restore = null;
  });

  function makeVisibleEl(absTop: number) {
    return {
      parentElement: null,
      getBoundingClientRect: () => ({
        width: 200, height: 200, top: absTop - container.scrollTop, bottom: absTop - container.scrollTop + 200, left: 0, right: 200,
      }),
      classList: { add: () => {}, remove: () => {} },
    } as any;
  }

  /** Arm the follow, then land a link on an old turn, both settled. */
  function armThenLandOnAnOldTurn() {
    restore = installFakeDom({ dataEventMatches: [makeVisibleEl(3000)] });
    const observers = makeScrollObservers(container);
    setFollowLiveEdge(true);      // the reader arms the follow
    vi.advanceTimersByTime(1500); // its glide settles on the live edge
    expect(container.scrollTop).toBe(9200);
    scrollToEventAndPulse('e-7'); // then follows a link to an older turn
    vi.advanceTimersByTime(1500); // the landing settles, and the claim releases
    expect(container.scrollTop).toBe(3000);
    return observers;
  }

  it('turns the toggle off, because the reader named a place', () => {
    armThenLandOnAnOldTurn();
    expect(followingLiveEdge.value).toBe(false);
  });

  it('is not undone by the growth the link itself causes', () => {
    // The deep-link renders the FULL exchange list so a windowed-out target can
    // be found, which is a large resize arriving right around the landing. It
    // must not teleport the reader to the bottom one beat after they tapped the
    // notification.
    const { onResize } = armThenLandOnAnOldTurn();

    container.scrollHeight = 20000; // render-all, a decoded image, markdown settling
    onResize();

    expect(container.scrollTop).toBe(3000);
  });

  it('and the reader STAYS on the event when the thread wakes', () => {
    // The report, as a test. A question card is a quiescent thread that is about
    // to run: the reader answers, the agent picks it up, and this resize is the
    // first thing the woken turn produces. With the ride still armed it wrote
    // them to the live edge here. Now nothing does.
    const { onResize } = armThenLandOnAnOldTurn();

    setThreadLive(true);
    container.scrollHeight = 20000;
    onResize();

    expect(container.scrollTop).toBe(3000);
  });
});

describe('deep-link deadline: a dead link reports without moving the transcript', () => {
  // The deadline used to expire in silence: claim released, nothing scrolled,
  // nothing said. A notification tap that hit an event this thread doesn't show
  // therefore looked simply broken. It reports the dead link through the
  // caller's `onUnresolved` (the words live in `store/actions/threads.ts`, which
  // owns the toast; scrollState stays free of the `store` import).
  //
  // The report is the WHOLE recovery. It used to also scroll to the thread's
  // most recent turn, guarded by a `watchUserAction` watcher so a reader who had
  // scrolled away meanwhile was not yanked 4s later. The user asked to go to a
  // place; the place does not exist, and the bottom is not it. So the scroll is
  // gone, and with it the watcher it needed: leaving the reader where they are
  // is the rule now, not the exception.
  let restore: (() => void) | null = null;
  let container: any;
  let onUnresolved: Mock<() => void>;

  beforeEach(() => {
    vi.useFakeTimers();
    container = makeContainer();
    setActiveScrollElement(container);
    onUnresolved = vi.fn<() => void>();
  });
  afterEach(() => {
    clearNavFocus();
    vi.clearAllTimers();
    vi.useRealTimers();
    setActiveScrollElement(null);
    restore?.();
    restore = null;
  });

  function makeVisibleEl(absTop = 3000) {
    return {
      parentElement: null,
      getBoundingClientRect: () => ({ width: 200, height: 200, top: absTop - container.scrollTop, bottom: absTop - container.scrollTop + 200, left: 0, right: 200 }),
      classList: { add: () => {}, remove: () => {} },
      querySelector: () => null,
    } as any;
  }

  function fireMutation() {
    const node = { nodeType: 1, matches: () => true, querySelector: () => null } as any;
    lastMoCallback?.([{ addedNodes: [node], type: 'childList' } as unknown as MutationRecord], {} as MutationObserver);
  }

  it('reports the failure and leaves the transcript exactly where it was', () => {
    restore = installFakeDom({}); // target never appears

    scrollToEventAndPulse('e-7', { onUnresolved });
    expect(container.scrollTop).toBe(0);

    vi.advanceTimersByTime(5000); // past EVENT_RESOLVE_DEADLINE_MS (4000)

    expect(container.scrollTop).toBe(0); // this used to land on the bottom
    expect(onUnresolved).toHaveBeenCalledTimes(1);
    // No pulse: there is no specific element to highlight, and marking the last
    // turn would claim the deep-link landed on it.
    expect(hasNavFocus()).toBe(false);
    expect(hasPendingEventScroll()).toBe(false);
  });

  it('leaves a reader who scrolled away during the wait exactly where they went', () => {
    // The case the retired `watchUserAction` guard existed for. It now holds
    // without a guard, because the deadline moves nobody at all.
    restore = installFakeDom({}); // target never appears

    scrollToEventAndPulse('e-7', { onUnresolved });
    container.scrollTop = 1234;

    vi.advanceTimersByTime(5000);

    expect(container.scrollTop).toBe(1234); // untouched: no late yank
    expect(onUnresolved).toHaveBeenCalledTimes(1); // still told, since nothing else says so
  });

  it('a synchronous resolve does not report', () => {
    restore = installFakeDom({ dataEventMatches: [makeVisibleEl()] });

    scrollToEventAndPulse('e-7', { onUnresolved });
    vi.advanceTimersByTime(6000); // well past the deadline the async path would have set

    expect(container.scrollTop).toBe(3000); // the event, not the bottom
    expect(onUnresolved).not.toHaveBeenCalled();
  });

  it('an observer resolve does not report, though its deadline still fires', () => {
    restore = installFakeDom({}); // not in the DOM yet: the async path

    scrollToEventAndPulse('e-7', { onUnresolved });
    expect(moObservations).toHaveLength(1);

    const visibleEl = makeVisibleEl();
    (globalThis.document as any).querySelectorAll = (sel: string) =>
      sel.startsWith('[data-event-id') ? [visibleEl] : [];
    fireMutation();

    // The deadline timer is still armed after an observer resolve (it doubles as
    // the claim's release), so it MUST distinguish "resolved" from "gave up".
    vi.advanceTimersByTime(6000);

    expect(container.scrollTop).toBe(3000); // the event, not the bottom
    expect(onUnresolved).not.toHaveBeenCalled();
  });

  it('runs the recovery exactly once, with the observer still attached at expiry', () => {
    restore = installFakeDom({}); // target never appears, so the observer is live at expiry

    scrollToEventAndPulse('e-7', { onUnresolved });
    expect(moObservations).toHaveLength(1);

    vi.advanceTimersByTime(5000);
    expect(onUnresolved).toHaveBeenCalledTimes(1);

    // Nothing re-arms it: later timer work must not produce a second recovery.
    vi.advanceTimersByTime(20000);
    expect(onUnresolved).toHaveBeenCalledTimes(1);
    expect(container.scrollTop).toBe(0);
  });

  it('re-tapping the SAME notification mid-wait does not let the older deadline recover over it', () => {
    // The claim is identified per CALL, not by what it points at. Two taps on
    // one notification inside the 4s window produce an identical target, so a
    // target-keyed claim let the first deadline mistake the second call's claim
    // for its own: it released the live claim, snapped to the bottom and
    // reported a dead link while the second attempt was still waiting, and the
    // second deadline then went silent because the claim no longer looked like
    // its own.
    restore = installFakeDom({}); // target never appears for either call

    scrollToEventAndPulse('e-7', { onUnresolved });
    vi.advanceTimersByTime(3000);
    scrollToEventAndPulse('e-7', { onUnresolved }); // same notification, tapped again

    // The FIRST call's deadline (t=4000) must stand down: the claim is the
    // second call's now, and that navigation is still live.
    vi.advanceTimersByTime(1500); // t=4500
    expect(onUnresolved).not.toHaveBeenCalled();
    expect(hasPendingEventScroll()).toBe(true);

    // The SECOND call's own deadline (t=3000+4000) is what reports, once.
    vi.advanceTimersByTime(3000); // t=7500
    expect(onUnresolved).toHaveBeenCalledTimes(1);
    expect(hasPendingEventScroll()).toBe(false);
  });

  it('the change deep-link reports the same way, off the same deadline', () => {
    restore = installFakeDom({}); // no matching turn in this thread

    scrollToChangeAndPulse('c-1', { onUnresolved });
    vi.advanceTimersByTime(5000);

    expect(container.scrollTop).toBe(0);
    expect(onUnresolved).toHaveBeenCalledTimes(1);
  });
});

describe('scrollToEventAndPulse — mobile header pin', () => {
  // Regression: on mobile a deep-link landed the event behind a half-hidden app
  // header ("covered a bit"). The deep-link scroll lands the event at the
  // container top minus its STATIC scroll-margin-top — only exact if the header's
  // visible portion is deterministic. The scroll therefore (a) reveals the header
  // now and (b) pins it visible for a short window so the smooth scroll-down can't
  // half-hide it.
  let restore: (() => void) | null = null;
  let container: any;

  beforeEach(() => {
    vi.useFakeTimers();
    container = makeContainer();
    setActiveScrollElement(container);
  });
  afterEach(() => {
    vi.clearAllTimers();
    vi.useRealTimers();
    setActiveScrollElement(null);
    restore?.();
    restore = null;
  });

  function makeVisibleEl(absTop = 3000) {
    const el: any = {
      parentElement: null,
      getBoundingClientRect: () => ({ width: 200, height: 200, top: absTop - container.scrollTop, bottom: absTop - container.scrollTop + 200, left: 0, right: 200 }),
      classList: { add: () => {}, remove: () => {} },
    };
    return el;
  }

  it('pins the header visible on resolve, dispatches reveal-mobile-header, then releases the pin after the scroll settles', () => {
    const visibleEl = makeVisibleEl();
    restore = installFakeDom({ dataEventMatches: [visibleEl] });

    let revealed = false;
    const onReveal = () => { revealed = true; };
    document.addEventListener('reveal-mobile-header', onReveal);

    expect(isHeaderPinnedForScroll()).toBe(false);
    scrollToEventAndPulse('e-7');

    // Reveal + pin happen synchronously on resolve, before the tween runs.
    expect(revealed).toBe(true);
    expect(isHeaderPinnedForScroll()).toBe(true);

    // The tween lands on the event within its (< HEADER_PIN_MS) duration, so the
    // header is still pinned when it arrives.
    vi.advanceTimersByTime(700); // tween done (≤ SCROLL_MAX_MS), still < HEADER_PIN_MS (800)
    expect(container.scrollTop).toBe(3000);
    expect(isHeaderPinnedForScroll()).toBe(true);

    // Pin is short-lived — it covers the smooth scroll, not the full deep-link
    // claim, so normal hide-on-scroll resumes once the user reads on.
    vi.advanceTimersByTime(200); // total 900, past HEADER_PIN_MS (800)
    expect(isHeaderPinnedForScroll()).toBe(false);

    document.removeEventListener('reveal-mobile-header', onReveal);
  });
});

describe('deep-link pulse — scoped to the subject panel, not the whole exchange', () => {
  // Regression: both data-event-id and data-change-id sit on the .chat-exchange
  // wrapper, which holds BOTH the .initiator-panel (the user message / event) AND
  // the .response-panel (the agent response) of the turn. Pulsing the whole
  // wrapper highlighted both. Each deep-link scopes the pulse to the panel that
  // holds its subject: an event → .initiator-panel (the event, not the response
  // below it); a change → .response-panel on a proposing CC turn (where the
  // ChangeProposed step lives), but .initiator-panel on a resolution card (which
  // carries the change body, recognised by its initiator-panel-change-* accent
  // class). A missing panel falls back to the whole target.
  let restore: (() => void) | null = null;

  beforeEach(() => { vi.useFakeTimers(); });
  afterEach(() => {
    vi.clearAllTimers();
    vi.useRealTimers();
    restore?.();
    restore = null;
  });

  function makeClassList() {
    const classes = new Set<string>();
    return {
      _classes: classes,
      add: (c: string) => { classes.add(c); },
      remove: (c: string) => { classes.delete(c); },
    };
  }

  /** An exchange element with a tracked classList and optional `.initiator-panel`
   *  / `.response-panel` children returned by querySelector — the two panels a
   *  deep-link pulse scopes to. `resolutionCard` makes the initiator match the
   *  `initiator-panel-change-*` accent probe, so the change picker treats it as a
   *  resolution card (→ .initiator-panel) instead of a proposing turn
   *  (→ .response-panel). No active scroll container is registered in this block,
   *  so the deep-link tween is a no-op here — these tests assert only the
   *  pulse-marker scoping. */
  function makeExchangeEl(
    { initiator = null, response = null, resolutionCard = false }:
      { initiator?: any; response?: any; resolutionCard?: boolean } = {},
  ) {
    const el: any = {
      parentElement: null,
      getBoundingClientRect: () => ({ width: 200, height: 200, top: 10, bottom: 210, left: 0, right: 200 }),
      classList: makeClassList(),
      // The event picker narrows to `.initiator-panel` only for a match that IS
      // the turn wrapper, so the fake has to answer the same question the real
      // element does. A step-level card (makeStepCardEl below) answers `false`
      // and is pulsed whole.
      matches: (sel: string) => sel === '.chat-exchange',
      querySelector: (sel: string) => {
        // The change picker probes for the resolution-card accent class first.
        if (sel.includes('initiator-panel-change-')) return resolutionCard ? initiator : null;
        if (sel === '.initiator-panel') return initiator;
        if (sel === '.response-panel') return response;
        return null;
      },
    };
    return el;
  }

  it('event deep-link marks the .initiator-panel, leaving the .chat-exchange wrapper unmarked', () => {
    const initiatorPanel = { classList: makeClassList() };
    const exchangeEl = makeExchangeEl({ initiator: initiatorPanel });
    restore = installFakeDom({ dataEventMatches: [exchangeEl] });

    scrollToEventAndPulse('e-7');

    expect(initiatorPanel.classList._classes.has('nav-focus-stuck')).toBe(true);
    expect(exchangeEl.classList._classes.has('nav-focus-stuck')).toBe(false);
  });

  it('event deep-link falls back to the whole exchange when no .initiator-panel is found', () => {
    const exchangeEl = makeExchangeEl(); // querySelector('.initiator-panel') → null
    restore = installFakeDom({ dataEventMatches: [exchangeEl] });

    scrollToEventAndPulse('e-7');

    expect(exchangeEl.classList._classes.has('nav-focus-stuck')).toBe(true);
  });

  it('change deep-link on a proposing turn marks the .response-panel, leaving the initiator + wrapper unmarked', () => {
    const initiatorPanel = { classList: makeClassList() };
    const responsePanel = { classList: makeClassList() };
    const exchangeEl = makeExchangeEl({ initiator: initiatorPanel, response: responsePanel });
    restore = installFakeDom({ dataEventMatches: [exchangeEl] });

    scrollToChangeAndPulse('c-1');

    expect(responsePanel.classList._classes.has('nav-focus-stuck')).toBe(true);
    // NOT the user message that started the turn, NOT the whole exchange wrapper.
    expect(initiatorPanel.classList._classes.has('nav-focus-stuck')).toBe(false);
    expect(exchangeEl.classList._classes.has('nav-focus-stuck')).toBe(false);
  });

  it('change deep-link on a resolution card marks the .initiator-panel (change body), not a folded-in continuation .response-panel', () => {
    // A ChangeApplied/Discarded/Reverted/Failed card carries the change body in
    // its .initiator-panel (accent class), and may fold post-apply continuation
    // work into a .response-panel. The pulse must land on the change body.
    const initiatorPanel = { classList: makeClassList() };
    const responsePanel = { classList: makeClassList() };
    const exchangeEl = makeExchangeEl({ initiator: initiatorPanel, response: responsePanel, resolutionCard: true });
    restore = installFakeDom({ dataEventMatches: [exchangeEl] });

    scrollToChangeAndPulse('c-1');

    expect(initiatorPanel.classList._classes.has('nav-focus-stuck')).toBe(true);
    expect(responsePanel.classList._classes.has('nav-focus-stuck')).toBe(false);
    expect(exchangeEl.classList._classes.has('nav-focus-stuck')).toBe(false);
  });

  it('change deep-link falls back to the whole exchange when the target panel is absent', () => {
    // Degenerate proposing turn: no .response-panel to scope to → whole target.
    const exchangeEl = makeExchangeEl({ initiator: { classList: makeClassList() } });
    restore = installFakeDom({ dataEventMatches: [exchangeEl] });

    scrollToChangeAndPulse('c-1');

    expect(exchangeEl.classList._classes.has('nav-focus-stuck')).toBe(true);
  });

  /** A step-level card: the rendered surface for an event that is folded into an
   *  exchange as a STEP rather than starting one (the `ResponseFailed` failure
   *  card, `.exchange-error`). It carries its own `data-event-id`, so it is what
   *  the deep-link matches, and it is NOT a `.chat-exchange`. The `querySelector`
   *  deliberately answers a panel for every probe: a card that gets narrowed
   *  anyway must fail loudly here rather than pass on "there was nothing inside
   *  to narrow to". */
  function makeStepCardEl(inner: any) {
    return {
      parentElement: null,
      getBoundingClientRect: () => ({ width: 200, height: 60, top: 10, bottom: 70, left: 0, right: 200 }),
      classList: makeClassList(),
      matches: (sel: string) => sel === '.exchange-error',
      querySelector: () => inner,
    } as any;
  }

  it('a ResponseFailed folded in as a step pulses its OWN card, with no narrowing into it', () => {
    // `ResponseFailed` is a terminal routed into the owning exchange by
    // request_event_id, not an EXCHANGE_START_TYPE, so the turn's root carries a
    // different event's id and the failure was unreachable. `ChatExchange` now
    // stamps the failure card itself, and the pulse must land on THAT card: not
    // the whole turn (a failure buried in a long turn is not "the turn"), and
    // not a descendant.
    const innerPanel = { classList: makeClassList() };
    const failureCard = makeStepCardEl(innerPanel);
    restore = installFakeDom({ dataEventMatches: [failureCard] });

    scrollToEventAndPulse('failed-evt');

    expect(failureCard.classList._classes.has('nav-focus-stuck')).toBe(true);
    expect(innerPanel.classList._classes.has('nav-focus-stuck')).toBe(false);
  });

  it('the exchange-start events that already navigated still resolve to the exchange root', () => {
    // Regression guard for the four event types whose deep-links worked before
    // step-level addressing existed. They are EXCHANGE_START_TYPES, so each
    // stamps `.chat-exchange` and must keep narrowing to its `.initiator-panel`.
    for (const eventId of [
      'user-question-asked',
      'coding-agent-permission-request',
      'credential-requested',
      'mcp-consent-requested',
    ]) {
      clearPendingEventScroll();
      const initiatorPanel = { classList: makeClassList() };
      const exchangeEl = makeExchangeEl({ initiator: initiatorPanel });
      restore?.();
      restore = installFakeDom({ dataEventMatches: [exchangeEl] });

      scrollToEventAndPulse(eventId);

      expect(initiatorPanel.classList._classes.has('nav-focus-stuck'), eventId).toBe(true);
      expect(exchangeEl.classList._classes.has('nav-focus-stuck'), eventId).toBe(false);
    }
  });

  it('dual-mount: the hidden layout copy of a failure card is skipped, the visible one is pulsed', () => {
    // Desktop and mobile each render the transcript, so BOTH copies carry the
    // new attribute exactly as they carry the exchange root's. The hidden one
    // reports a 0×0 rect and must lose the match, or the pulse runs invisibly.
    const hiddenCard = makeStepCardEl(null);
    hiddenCard.getBoundingClientRect = () => ({ width: 0, height: 0, top: 0, bottom: 0, left: 0, right: 0 });
    const visibleCard = makeStepCardEl(null);
    // Document order puts the hidden (desktop) copy first, which is what made
    // this worth pinning: a first-match resolve would take it.
    restore = installFakeDom({ dataEventMatches: [hiddenCard, visibleCard] });

    scrollToEventAndPulse('failed-evt');

    expect(visibleCard.classList._classes.has('nav-focus-stuck')).toBe(true);
    expect(hiddenCard.classList._classes.has('nav-focus-stuck')).toBe(false);
  });
});

describe('isEventInViewport for a step-level card', () => {
  // The notification §4 in-app matrix asks "is the user already looking at the
  // thing this notification points at?" and silently marks it read when so. It
  // resolves the source event through the same `data-event-id`, so stamping the
  // failure card is what makes the question answerable for a `ResponseFailed`;
  // before, the query found nothing and the answer was always false.
  let restore: (() => void) | null = null;
  let container: any;

  beforeEach(() => {
    container = makeContainer();
    setActiveScrollElement(container);
  });
  afterEach(() => {
    setActiveScrollElement(null);
    restore?.();
    restore = null;
  });

  /** A failure card whose rect sits `top` px down the 0..800 scroll container. */
  function makeCardAt(top: number) {
    return {
      parentElement: null,
      getBoundingClientRect: () => ({ width: 200, height: 60, top, bottom: top + 60, left: 0, right: 200 }),
      classList: { add: () => {}, remove: () => {} },
    } as any;
  }

  it('true when the failure card is on screen', () => {
    restore = installFakeDom({ dataEventMatches: [makeCardAt(300)] });
    expect(isEventInViewport('failed-evt')).toBe(true);
  });

  it('false when the failure card is scrolled out of the transcript band', () => {
    // Inside window.innerHeight is not enough: the band is the transcript
    // container's (0..800 here), so a card below it is not on screen.
    restore = installFakeDom({ dataEventMatches: [makeCardAt(2400)] });
    expect(isEventInViewport('failed-evt')).toBe(false);
  });

  it('false when the only copy is the hidden layout mount', () => {
    const hidden = makeCardAt(300);
    hidden.getBoundingClientRect = () => ({ width: 0, height: 0, top: 0, bottom: 0, left: 0, right: 0 });
    restore = installFakeDom({ dataEventMatches: [hidden] });
    expect(isEventInViewport('failed-evt')).toBe(false);
  });
});

describe('scrollToChangeAndPulse — resolves to the LAST visible match', () => {
  // An applied change appears twice in the thread: the proposing CC turn
  // (ChangeProposed rides it as a step) earlier, and the ChangeApplied
  // resolution card later. Both carry the same data-change-id, so first-match
  // would land on the CC turn — the reported "doesn't scroll to the change
  // applied event" bug. scrollToChangeAndPulse must prefer the last (resolution
  // card); a pending change has only the one (proposing) match.
  let restore: (() => void) | null = null;
  let container: any;
  beforeEach(() => {
    vi.useFakeTimers();
    container = makeContainer();
    setActiveScrollElement(container);
  });
  afterEach(() => {
    vi.clearAllTimers();
    vi.useRealTimers();
    setActiveScrollElement(null);
    restore?.();
    restore = null;
  });

  // Each match lands the tween at its own absTop, so the resulting container
  // scrollTop tells us WHICH match was chosen.
  function makeVisibleEl(absTop: number) {
    const el: any = {
      parentElement: null,
      getBoundingClientRect: () => ({ width: 200, height: 200, top: absTop - container.scrollTop, bottom: absTop - container.scrollTop + 200, left: 0, right: 200 }),
      classList: { add: () => {}, remove: () => {} },
    };
    return el;
  }

  it('applied change → lands on the resolution card (last), not the proposing turn (first)', () => {
    const proposingTurn = makeVisibleEl(1000);
    const appliedCard = makeVisibleEl(5000);
    // Document order: proposing turn first, resolution card last.
    restore = installFakeDom({ dataEventMatches: [proposingTurn, appliedCard] });

    scrollToChangeAndPulse('c-1');

    // The tween lands on the appliedCard's position (5000), not the proposing
    // turn's (1000) — preferLast picked the resolution card.
    vi.advanceTimersByTime(800); // past the tween duration (≤ SCROLL_MAX_MS)
    expect(container.scrollTop).toBe(5000);
    // Synchronous resolve now holds the claim across the smooth-scroll settle,
    // then releases on the fallback timer (same contract as the event deep-link).
    expect(hasPendingEventScroll()).toBe(true);
    vi.advanceTimersByTime(300); // total 1100, past SCROLL_SETTLE_FALLBACK_MS (1000)
    expect(hasPendingEventScroll()).toBe(false);
  });

  it('pending change → lands on its single (proposing) match', () => {
    const proposingTurn = makeVisibleEl(2000);
    restore = installFakeDom({ dataEventMatches: [proposingTurn] });

    scrollToChangeAndPulse('c-1');

    vi.advanceTimersByTime(800);
    expect(container.scrollTop).toBe(2000);
  });
});

describe('chat deep-link applies the shared navigation focus marker', () => {
  // The chat deep-link routes its highlight through the shared focus marker
  // (components/shared/focusMarker.ts): a sticky background wash with a spotlight
  // glow. The marker's own behaviors (supersede, gesture-clear semantics) are
  // covered in focusMarker.test.ts, and its paint by nav-focus-marker-paint.test.ts.
  // These tests pin the CHAT integration: the
  // marker is applied on resolve, survives the landing scroll, clears on
  // clearPendingEventScroll, and (the chat-specific bit) its gesture-clear is
  // gated on the deep-link claim (settleGuard = hasPendingEventScroll) so the
  // landing scroll can't self-clear it.
  let restore: (() => void) | null = null;
  beforeEach(() => { vi.useFakeTimers(); });
  afterEach(() => {
    clearNavFocus(); // tear down any armed document gesture listeners
    vi.clearAllTimers();
    vi.useRealTimers();
    restore?.();
    restore = null;
  });

  /** A visible element with a tracked classList and a querySelector that returns
   *  null, so scrollToEventAndPulse's `.initiator-panel` lookup falls back to the
   *  element itself (the marker lands on it). No active scroll container is
   *  registered here, so the deep-link tween is a no-op — these tests assert only
   *  the marker lifecycle. */
  function makeMarkerEl() {
    const classes = new Set<string>();
    const el: any = {
      parentElement: null,
      offsetWidth: 0,
      getBoundingClientRect: () => ({ width: 200, height: 200, top: 10, bottom: 210, left: 0, right: 200 }),
      classList: { _classes: classes, add: (c: string) => classes.add(c), remove: (c: string) => classes.delete(c) },
      querySelector: () => null,
    };
    return el;
  }

  it('applies the sticky highlight on resolve', () => {
    const el = makeMarkerEl();
    restore = installFakeDom({ dataEventMatches: [el] });

    scrollToEventAndPulse('e-7');
    expect(el.classList._classes.has('nav-focus-stuck')).toBe(true);
    expect(el.classList._classes.has('nav-focus-fading')).toBe(false);
    expect(hasNavFocus()).toBe(true);
  });

  it('clearPendingEventScroll() drops the marker (a plain focus / explicit scroll)', () => {
    const el = makeMarkerEl();
    restore = installFakeDom({ dataEventMatches: [el] });
    scrollToEventAndPulse('e-7');
    expect(hasNavFocus()).toBe(true);

    clearPendingEventScroll();
    expect(el.classList._classes.has('nav-focus-stuck')).toBe(false);
    expect(hasNavFocus()).toBe(false);
  });

  it('dismissal is gated on the deep-link claim — deferred while held, fades after it settles', () => {
    const el = makeMarkerEl();
    restore = installFakeDom({ dataEventMatches: [el] });
    scrollToEventAndPulse('e-7');
    expect(hasNavFocus()).toBe(true);

    // Claim still held (the sync resolve holds it across the smooth-scroll settle)
    // → an action is the programmatic landing scroll, not the user, so it's ignored.
    document.dispatchEvent(new Event('wheel'));
    expect(hasNavFocus()).toBe(true);
    expect(el.classList._classes.has('nav-focus-fading')).toBe(false);

    vi.advanceTimersByTime(1100); // past SCROLL_SETTLE_FALLBACK_MS (1000) → claim released
    expect(hasPendingEventScroll()).toBe(false);

    // Past the marker's hold too, so what this test observes is the
    // CLAIM gating the dismissal and not the hold standing in for it. The two defer
    // for different reasons and only the claim is under test here.
    vi.advanceTimersByTime(NAV_FOCUS_RAMP_MS + NAV_FOCUS_HOLD_MS);

    // THE assertion that makes this test about the claim. The guarded wheel above has
    // to have been DISCARDED, not banked: the hold banks a real dismissal and runs it
    // when it expires, which the advance just crossed, so without this line deleting
    // the settleGuard wiring entirely leaves the whole spec green (verified). What the
    // guard uniquely buys is that a landing scroll never enters the bank at all.
    expect(el.classList._classes.has('nav-focus-fading')).toBe(false);

    // Now an action is the user engaging → marker dissolves, then is removed.
    document.dispatchEvent(new Event('wheel'));
    expect(el.classList._classes.has('nav-focus-fading')).toBe(true);
    vi.advanceTimersByTime(NAV_FOCUS_FADE_MS);
    expect(el.classList._classes.has('nav-focus-stuck')).toBe(false);
    expect(hasNavFocus()).toBe(false);
  });
});
