import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

import { scrollToEventAndPulse, scrollToChangeAndPulse, hasPendingEventScroll, clearPendingEventScroll, scrollToBottom, scrolledUp, isHeaderPinnedForScroll, setActiveScrollElement } from '../scrollState';
import { hasNavFocus, clearNavFocus, NAV_FOCUS_FADE_MS } from '../../shared/focusMarker';

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
  // events; the scroll-to-bottom that fires on the eventsLoaded false→true
  // transition (ThreadView + useAutoScroll) overrode the deep-link scroll the
  // moment the events rendered. The fix exposes a "pending event scroll"
  // flag those callers consult to defer until scrollToEventAndPulse lands.
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
    scrolledUp.value = false;

    scrollToEventAndPulse('e-7');

    // The claim is NOT released synchronously — the deep-link tween is still
    // settling, and a competing scroll (panel close, iOS viewport reflow)
    // in that window would override the landing. It's held until scrollend / the
    // fallback timer so every auto-scroll path keeps deferring across the scroll.
    expect(hasPendingEventScroll()).toBe(true);
    // Parked on a mid-thread event → pinned so the next render's auto-scroll
    // defers instead of snapping back to the bottom.
    expect(scrolledUp.value).toBe(true);
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

  it('an AUTOMATIC scroll-to-bottom during the lazy load does NOT clear the claim, and the target still resolves + pulses when it renders (cross-thread/unfocused path)', () => {
    restore = installFakeDom({}); // target not in the DOM yet (unfocused thread)
    scrolledUp.value = false;

    scrollToEventAndPulse('e-7');
    expect(hasPendingEventScroll()).toBe(true);
    expect(moObservations).toHaveLength(1);

    // The eventsLoaded false→true transition fires the lazy-load auto-scroll
    // WHILE the deep-link is still waiting for its target to render. As an
    // automatic scroll it must DEFER to the claim — not clear it (the regression:
    // clearing here un-guarded every other auto-scroll path and landed the user
    // at the bottom). Repeated fires during a slow load must all be no-ops.
    scrollToBottom({ auto: true });
    scrollToBottom({ auto: true });
    expect(hasPendingEventScroll()).toBe(true); // claim survived the auto-scrolls

    // Events finally render: the target card appears and is visible.
    const visibleEl = makeVisibleEl();
    (globalThis.document as any).querySelectorAll = (sel: string) =>
      sel.startsWith('[data-event-id') ? [visibleEl] : [];
    fireMutation([makeTargetNode()]);

    // The deep-link still lands on (and pins) the event — the auto-scrolls never
    // stole the target.
    vi.advanceTimersByTime(800); // run the tween
    expect(container.scrollTop).toBe(3000);
    expect(scrolledUp.value).toBe(true);
    expect(hasPendingEventScroll()).toBe(true); // held until the deadline

    vi.advanceTimersByTime(5000); // past EVENT_RESOLVE_DEADLINE_MS (4000)
    expect(hasPendingEventScroll()).toBe(false);
  });

  it('stays pending while the lazily-loaded event has not rendered (unfocused-thread path), scrolls + pins on resolve, and HOLDS the claim until the deadline', () => {
    restore = installFakeDom({}); // target not in the DOM yet
    scrolledUp.value = false;

    scrollToEventAndPulse('e-7');
    // This is the flag ThreadView/useAutoScroll consult to defer their
    // scroll-to-bottom so it can't override the upcoming deep-link scroll.
    expect(hasPendingEventScroll()).toBe(true);
    expect(moObservations).toHaveLength(1);

    // Events render: the target card appears and is visible.
    const visibleEl = makeVisibleEl();
    (globalThis.document as any).querySelectorAll = (sel: string) =>
      sel.startsWith('[data-event-id') ? [visibleEl] : [];
    fireMutation([makeTargetNode()]);

    vi.advanceTimersByTime(800); // run the tween → lands on the event
    expect(container.scrollTop).toBe(3000);
    expect(scrolledUp.value).toBe(true);
    // The claim is HELD past the scroll — the same render that revealed the
    // event also re-fires the events-load effect + a late resize pin, which the
    // claim must keep suppressed. It releases only at the deadline.
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

  it('scrollToBottom() supersedes an in-flight claim (e.g. answering a deep-linked question)', () => {
    // An explicit go-to-bottom (sending a follow-up while a deep-link claim is
    // still held) must release the claim so the streamed response can tail.
    restore = installFakeDom({}); // target never appears → claim stays pending
    scrollToEventAndPulse('e-7');
    expect(hasPendingEventScroll()).toBe(true);

    scrollToBottom();
    expect(hasPendingEventScroll()).toBe(false);
  });

  it('clears the pending flag when the deadline passes without the event, and does NOT yank the scroll', () => {
    restore = installFakeDom({}); // target never appears
    // Simulate the user (or onResize during load) having parked off-bottom
    // while the deep-link waited. The deadline must release the claim WITHOUT
    // forcing a scroll-to-bottom that would yank the user away.
    scrolledUp.value = true;

    scrollToEventAndPulse('e-7');
    expect(hasPendingEventScroll()).toBe(true);

    vi.advanceTimersByTime(5000); // past EVENT_RESOLVE_DEADLINE_MS (4000)

    expect(hasPendingEventScroll()).toBe(false);
    expect(scrolledUp.value).toBe(true); // untouched — no scroll-to-bottom yank
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
  // (components/shared/focusMarker.ts): a fill flash that fades, leaving a sticky
  // border. The marker's own behaviors (supersede, gesture-clear semantics) are
  // covered in focusMarker.test.ts; these tests pin the CHAT integration — the
  // marker is applied on resolve, survives the flash, clears on
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

  it('applies the sticky border on resolve (no entrance animation)', () => {
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

    // Now an action is the user engaging → marker fades out, then is removed.
    document.dispatchEvent(new Event('wheel'));
    expect(el.classList._classes.has('nav-focus-fading')).toBe(true);
    vi.advanceTimersByTime(NAV_FOCUS_FADE_MS);
    expect(el.classList._classes.has('nav-focus-stuck')).toBe(false);
    expect(hasNavFocus()).toBe(false);
  });
});
