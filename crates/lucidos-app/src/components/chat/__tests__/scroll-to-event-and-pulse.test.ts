import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

import { scrollToEventAndPulse, scrollToChangeAndPulse, hasPendingEventScroll, clearPendingEventScroll, scrollToBottom, scrolledUp, isHeaderPinnedForScroll } from '../scrollState';

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

  return () => {
    (globalThis.document as any).body = orig.documentBody;
    (globalThis.document as any).querySelectorAll = orig.documentQSA;
    (globalThis as any).MutationObserver = orig.MutationObserver;
    (globalThis as any).CSS = orig.CSS;
  };
}

beforeEach(() => {
  moObservations.length = 0;
  lastMoCallback = null;
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
  // transition (ThreadView + useAutoScroll) overrode scrollIntoView the
  // moment the events rendered. The fix exposes a "pending event scroll"
  // flag those callers consult to defer until scrollToEventAndPulse lands.
  let restore: (() => void) | null = null;

  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.clearAllTimers();
    vi.useRealTimers();
    restore?.();
    restore = null;
  });

  /** A DOM element that passes isElementVisible (non-zero rect, no clipping
   *  ancestor) and records whether it was scrolled into view. */
  function makeVisibleEl() {
    const el: any = {
      parentElement: null,
      getBoundingClientRect: () => ({ width: 200, height: 200, top: 10, bottom: 210, left: 0, right: 200 }),
      classList: { add: () => {}, remove: () => {} },
      scrollIntoView: () => { el._scrolledIntoView = true; },
      _scrolledIntoView: false,
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

  it('resolves synchronously when the event is already in the DOM (focused-thread path) — never leaves a pending flag', () => {
    const visibleEl = makeVisibleEl();
    restore = installFakeDom({ dataEventMatches: [visibleEl] });
    scrolledUp.value = false;

    scrollToEventAndPulse('e-7');

    expect(visibleEl._scrolledIntoView).toBe(true);
    expect(hasPendingEventScroll()).toBe(false);
    // Parked on a mid-thread event → pinned so the next render's auto-scroll
    // defers instead of snapping back to the bottom.
    expect(scrolledUp.value).toBe(true);
    // Resolved before ever observing — no need to wait for lazily-loaded events.
    expect(moObservations).toHaveLength(0);
  });

  it('stays pending while the lazily-loaded event has not rendered (unfocused-thread path), scrolls + pins on resolve, and HOLDS the claim until the deadline', () => {
    restore = installFakeDom({}); // target not in the DOM yet
    scrolledUp.value = false;

    scrollToEventAndPulse('e-7');
    // This is the flag ThreadView/useAutoScroll consult to defer their
    // scroll-to-bottom so it can't override the upcoming scrollIntoView.
    expect(hasPendingEventScroll()).toBe(true);
    expect(moObservations).toHaveLength(1);

    // Events render: the target card appears and is visible.
    const visibleEl = makeVisibleEl();
    (globalThis.document as any).querySelectorAll = (sel: string) =>
      sel.startsWith('[data-event-id') ? [visibleEl] : [];
    fireMutation([makeTargetNode()]);

    expect(visibleEl._scrolledIntoView).toBe(true);
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
  // Regression: on mobile a deep-link scrollIntoView landed the event behind a
  // half-hidden app header ("covered a bit"). scrollIntoView ignores the
  // fixed/sticky chrome, so the landing relies on .chat-exchange's STATIC
  // scroll-margin-top — which is only exact if the header's visible portion is
  // deterministic. The scroll therefore (a) reveals the header now and (b) pins
  // it visible for a short window so the smooth scroll-down can't half-hide it.
  let restore: (() => void) | null = null;

  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.clearAllTimers();
    vi.useRealTimers();
    restore?.();
    restore = null;
  });

  function makeVisibleEl() {
    const el: any = {
      parentElement: null,
      getBoundingClientRect: () => ({ width: 200, height: 200, top: 10, bottom: 210, left: 0, right: 200 }),
      classList: { add: () => {}, remove: () => {} },
      scrollIntoView: () => { el._scrolledIntoView = true; },
      _scrolledIntoView: false,
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

    expect(visibleEl._scrolledIntoView).toBe(true);
    expect(revealed).toBe(true);
    expect(isHeaderPinnedForScroll()).toBe(true);

    // Pin is short-lived — it covers the smooth scroll, not the full deep-link
    // claim, so normal hide-on-scroll resumes once the user reads on.
    vi.advanceTimersByTime(900); // past HEADER_PIN_MS (800)
    expect(isHeaderPinnedForScroll()).toBe(false);

    document.removeEventListener('reveal-mobile-header', onReveal);
  });
});

describe('deep-link pulse — scoped to the event, not the response below', () => {
  // Regression: data-event-id sits on the .chat-exchange wrapper, which holds
  // BOTH the event (the .initiator-panel — e.g. a question card) AND the agent
  // response below it. Pulsing the whole wrapper highlighted the response too.
  // An event deep-link must scope the pulse to the .initiator-panel; a change
  // deep-link keeps highlighting the whole turn.
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

  /** An exchange element with a tracked classList and an optional
   *  `.initiator-panel` child returned by querySelector. */
  function makeExchangeEl(initiatorPanel: any = null) {
    const el: any = {
      parentElement: null,
      getBoundingClientRect: () => ({ width: 200, height: 200, top: 10, bottom: 210, left: 0, right: 200 }),
      classList: makeClassList(),
      querySelector: (sel: string) => (sel === '.initiator-panel' ? initiatorPanel : null),
      scrollIntoView: () => {},
    };
    return el;
  }

  it('event deep-link pulses the .initiator-panel, leaving the .chat-exchange wrapper unpulsed', () => {
    const initiatorPanel = { classList: makeClassList() };
    const exchangeEl = makeExchangeEl(initiatorPanel);
    restore = installFakeDom({ dataEventMatches: [exchangeEl] });

    scrollToEventAndPulse('e-7');

    expect(initiatorPanel.classList._classes.has('event-pulse')).toBe(true);
    expect(exchangeEl.classList._classes.has('event-pulse')).toBe(false);

    // Removed when the pulse window elapses — off the same element it landed on.
    vi.advanceTimersByTime(1800); // EVENT_PULSE_MS
    expect(initiatorPanel.classList._classes.has('event-pulse')).toBe(false);
  });

  it('event deep-link falls back to the whole exchange when no .initiator-panel is found', () => {
    const exchangeEl = makeExchangeEl(null); // querySelector('.initiator-panel') → null
    restore = installFakeDom({ dataEventMatches: [exchangeEl] });

    scrollToEventAndPulse('e-7');

    expect(exchangeEl.classList._classes.has('event-pulse')).toBe(true);
  });

  it('change deep-link pulses the whole .chat-exchange turn (not narrowed to the initiator)', () => {
    const initiatorPanel = { classList: makeClassList() };
    const exchangeEl = makeExchangeEl(initiatorPanel);
    restore = installFakeDom({ dataEventMatches: [exchangeEl] });

    scrollToChangeAndPulse('c-1');

    expect(exchangeEl.classList._classes.has('event-pulse')).toBe(true);
    expect(initiatorPanel.classList._classes.has('event-pulse')).toBe(false);
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
  afterEach(() => { restore?.(); restore = null; });

  function makeVisibleEl(tag: string) {
    const el: any = {
      _tag: tag,
      parentElement: null,
      getBoundingClientRect: () => ({ width: 200, height: 200, top: 10, bottom: 210, left: 0, right: 200 }),
      classList: { add: () => {}, remove: () => {} },
      scrollIntoView: () => { el._scrolledIntoView = true; },
      _scrolledIntoView: false,
    };
    return el;
  }

  it('applied change → lands on the resolution card (last), not the proposing turn (first)', () => {
    const proposingTurn = makeVisibleEl('proposed');
    const appliedCard = makeVisibleEl('applied');
    // Document order: proposing turn first, resolution card last.
    restore = installFakeDom({ dataEventMatches: [proposingTurn, appliedCard] });

    scrollToChangeAndPulse('c-1');

    expect(appliedCard._scrolledIntoView).toBe(true);
    expect(proposingTurn._scrolledIntoView).toBe(false);
    expect(hasPendingEventScroll()).toBe(false); // synchronous resolve
  });

  it('pending change → lands on its single (proposing) match', () => {
    const proposingTurn = makeVisibleEl('proposed');
    restore = installFakeDom({ dataEventMatches: [proposingTurn] });

    scrollToChangeAndPulse('c-1');

    expect(proposingTurn._scrolledIntoView).toBe(true);
  });
});
