import { describe, it, expect, beforeEach, afterEach, vi, type Mock } from 'vitest';

import { scrollToEventAndPulse, scrollToChangeAndPulse, hasPendingEventScroll, clearPendingEventScroll, isFollowScroll, makeScrollObservers, scrollToBottom, isEventInViewport, isHeaderPinnedForScroll, setActiveScrollElement, setFollowLiveEdge, stopFollowingBottom } from '../scrollState';
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

describe('a deep-link landing retires a standing follow', () => {
  /** Going to a link is the reader asking to be at ONE place, so the ride ends
   *  there. The scroll disarm cannot do this on its own: it needs the reader to
   *  be off the live edge AND away from where the follow last wrote, and the
   *  ordinary link into a thread the reader is riding points at its newest turn,
   *  which lands them precisely ON the live edge. So the follow survived the
   *  landing and the next token carried them off the event they had just asked
   *  to see. The counterpart lives in `scroll-follow-the-live-edge.test.ts`,
   *  which pins what does and does not ARM the same follow. */
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
    stopFollowingBottom();
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

  it('leaves the reader on the event even when the link lands ON the live edge', () => {
    // The blind spot, reproduced: the target is the thread's newest turn, so
    // the landing writes the same offset the follow was already holding and no
    // scroll event can read as the reader leaving.
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

    container.scrollHeight = 20000;   // the agent keeps working
    onResize();

    expect(container.scrollTop).toBe(9200);
    expect(isFollowScroll(container)).toBe(false);
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
