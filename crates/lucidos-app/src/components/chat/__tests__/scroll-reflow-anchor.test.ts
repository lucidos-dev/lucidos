import { describe, it, expect, beforeEach, vi } from 'vitest';

if (typeof globalThis.HTMLElement === 'undefined') {
  (globalThis as any).HTMLElement = class HTMLElement {};
}
if (typeof globalThis.document !== 'undefined' && !('activeElement' in globalThis.document)) {
  (globalThis.document as any).activeElement = null;
}
if (typeof globalThis.requestAnimationFrame === 'undefined') {
  (globalThis as any).requestAnimationFrame = (cb: any) => { cb(); return 0; };
}
if (typeof globalThis.cancelAnimationFrame === 'undefined') {
  (globalThis as any).cancelAnimationFrame = () => {};
}

import {
  awayFromBottom,
  clearPendingEventScroll,
  followingLiveEdge,
  hasPendingEventScroll,
  makeScrollObservers,
  notAtTop,
  readerGestureForTest,
  scrollToEventAndPulse,
  scrolledFromTop,
  setActiveScrollElement,
  setFollowLiveEdge,
  setTranscriptLive,
  stopFollowingBottom,
} from '../scrollState';

function installDeepLinkStubs() {
  const orig = {
    MutationObserver: (globalThis as any).MutationObserver,
    CSS: (globalThis as any).CSS,
    body: (globalThis.document as any).body,
  };
  (globalThis as any).MutationObserver = class {
    observe() {}
    disconnect() {}
    takeRecords() { return []; }
  };
  (globalThis as any).CSS = { escape: (s: string) => s };
  (globalThis.document as any).body = { tagName: 'BODY' };
  return () => {
    (globalThis as any).MutationObserver = orig.MutationObserver;
    (globalThis as any).CSS = orig.CSS;
    (globalThis.document as any).body = orig.body;
  };
}

function makeTranscript(opts: { turns: number[]; width: number; clientHeight: number; feed?: boolean; blocks?: number }) {
  const baseWidth = opts.width;
  const turns = [...opts.turns];
  let width = opts.width;
  let clientHeight = opts.clientHeight;
  let scrollTop = 0;

  const heightOf = (i: number) => (turns[i] * baseWidth) / width;
  const docTop = (i: number) => {
    let sum = 0;
    for (let n = 0; n < i; n++) sum += heightOf(n);
    return sum;
  };

  const blockCount = opts.blocks ?? 0;
  const blockTop = (i: number, k: number) => docTop(i) + (k * heightOf(i)) / blockCount - scrollTop;
  const turnEls = turns.map((_, i) => ({
    isConnected: true,
    children: Array.from({ length: blockCount }, (_, k) => ({
      isConnected: true,
      children: [],
      getBoundingClientRect: () => ({ top: blockTop(i, k), height: heightOf(i) / blockCount }),
    })),
    getBoundingClientRect: () => ({ top: docTop(i) - scrollTop, height: heightOf(i) }),
  }));
  const feedEl = {
    isConnected: true,
    children: turnEls,
    getBoundingClientRect: () => ({ top: -scrollTop, height: docTop(turns.length) }),
  };

  const el: any = {
    parentElement: null,
    children: opts.feed ? [feedEl] : turnEls,
    querySelector: (sel: string) => (opts.feed && sel === '.thread-feed' ? feedEl : null),
    get clientWidth() { return width; },
    get clientHeight() { return clientHeight; },
    get scrollHeight() { return docTop(turns.length); },
    get scrollTop() { return scrollTop; },
    set scrollTop(next: number) {
      const max = Math.max(0, el.scrollHeight - el.clientHeight);
      scrollTop = Math.max(0, Math.min(next, max));
    },
    getBoundingClientRect: () => ({ top: 0, left: 0, width, height: clientHeight }),

    setWidth(next: number) { width = next; el.scrollTop = scrollTop; },
    rotate(nextWidth: number, nextHeight: number) {
      width = nextWidth;
      clientHeight = nextHeight;
      el.scrollTop = scrollTop;
    },
    liveEdge() { return Math.max(0, el.scrollHeight - el.clientHeight); },
    growLastTurn(px: number) { turns[turns.length - 1] += px; el.scrollTop = scrollTop; },
    turnTop(i: number) { return docTop(i) - scrollTop; },
    blockTop,
  };
  return el;
}

const SIX_TURNS = { turns: [600, 600, 600, 600, 600, 600], width: 800, clientHeight: 600 };

const PORTRAIT = { turns: [600, 600, 600, 600, 600, 600], width: 400, clientHeight: 800 };

beforeEach(() => {
  awayFromBottom.value = false;
  notAtTop.value = false;
  scrolledFromTop.value = false;
  stopFollowingBottom();
  setTranscriptLive(true);
  readerGestureForTest(null, false);
  setActiveScrollElement(null);
});

describe('transcript reflow anchoring across a pane-width change', () => {
  it('keeps the turn the reader is on in place when the pane narrows', () => {
    const el = makeTranscript(SIX_TURNS);
    const { onScroll, onResize } = makeScrollObservers(el);

    el.scrollTop = 1900;
    onScroll();
    onResize();
    expect(el.turnTop(3)).toBe(-100);

    el.setWidth(400);
    onResize();

    expect(el.turnTop(3)).toBe(-100);
    expect(el.scrollTop).toBe(3700);
  });

  it('keeps the paragraph the reader is on in place, not only its turn', () => {
    const el = makeTranscript({ ...SIX_TURNS, feed: true, blocks: 6 });
    const { onScroll, onResize } = makeScrollObservers(el);

    el.scrollTop = 1950;
    onScroll();
    onResize();
    expect(el.blockTop(3, 1)).toBe(-50);

    el.setWidth(400);
    onResize();

    expect(el.blockTop(3, 1)).toBe(-50);
  });

  it('keeps the turn in place once a re-set reply replaced the paragraph', () => {
    const el = makeTranscript({ ...SIX_TURNS, feed: true, blocks: 6 });
    const { onScroll, onResize } = makeScrollObservers(el);

    el.scrollTop = 1950;
    onScroll();
    onResize();
    const turnTop = el.turnTop(3);
    for (const block of el.children[0].children[3].children) block.isConnected = false;

    el.setWidth(400);
    onResize();

    expect(el.turnTop(3)).toBe(turnTop);
  });

  it('anchors on a turn inside the feed box, not on the feed itself', () => {
    const el = makeTranscript({ ...SIX_TURNS, feed: true });
    const { onScroll, onResize } = makeScrollObservers(el);

    el.scrollTop = 1900;
    onScroll();
    onResize();
    expect(el.turnTop(3)).toBe(-100);

    el.setWidth(400);
    onResize();

    expect(el.turnTop(3)).toBe(-100);
    expect(el.scrollTop).toBe(3700);
  });

  it('keeps it in place when the pane widens and the browser clamps scrollTop', () => {
    const el = makeTranscript({ ...SIX_TURNS, width: 400 });
    const { onScroll, onResize } = makeScrollObservers(el);

    el.scrollTop = 1900;
    onScroll();
    onResize();
    expect(el.turnTop(3)).toBe(-100);

    el.setWidth(800);
    expect(el.scrollTop).toBe(1200);
    onResize();

    expect(el.turnTop(3)).toBe(-100);
    expect(el.scrollTop).toBe(1000);
  });

  it('anchors a reader sitting at the bottom too, rather than re-pinning them', () => {
    const el = makeTranscript(SIX_TURNS);
    const { onScroll, onResize } = makeScrollObservers(el);

    el.scrollTop = 3000;
    onScroll();
    expect(awayFromBottom.value).toBe(false);
    onResize();
    expect(el.turnTop(5)).toBe(0);

    el.setWidth(400);
    onResize();

    expect(el.turnTop(5)).toBe(0);
    expect(el.scrollTop).toBe(6000);
    expect(awayFromBottom.value).toBe(true);
  });

  it('holds the anchor while a deep-link is resolving', () => {
    const restoreDom = installDeepLinkStubs();
    vi.useFakeTimers();
    try {
      const el = makeTranscript(SIX_TURNS);
      const { onScroll, onResize } = makeScrollObservers(el);

      el.scrollTop = 1900;
      onScroll();
      onResize();

      scrollToEventAndPulse('never-renders');
      expect(hasPendingEventScroll()).toBe(true);

      el.setWidth(400);
      onResize();

      expect(el.scrollTop).not.toBe(6600);
      expect(el.turnTop(3)).toBe(-100);
    } finally {
      clearPendingEventScroll();
      vi.runOnlyPendingTimers();
      vi.useRealTimers();
      restoreDom();
    }
  });

  it('follows the reader when they scroll to a different turn', () => {
    const el = makeTranscript(SIX_TURNS);
    const { onScroll, onResize } = makeScrollObservers(el);

    el.scrollTop = 1900;
    onScroll();
    onResize();

    el.scrollTop = 1200;
    onScroll();
    expect(el.turnTop(2)).toBe(0);

    el.setWidth(400);
    onResize();

    expect(el.turnTop(2)).toBe(0);
    expect(el.scrollTop).toBe(2400);
  });

  it('leaves a reader at the very top at the very top', () => {
    const el = makeTranscript(SIX_TURNS);
    const { onScroll, onResize } = makeScrollObservers(el);

    el.scrollTop = 0;
    onScroll();
    onResize();

    el.setWidth(400);
    onResize();

    expect(el.scrollTop).toBe(0);
  });

  it('does not reposition on height-only growth (streaming)', () => {
    const el = makeTranscript(SIX_TURNS);
    const { onScroll, onResize } = makeScrollObservers(el);

    el.scrollTop = 1900;
    onScroll();
    onResize();

    el.growLastTurn(400);
    onResize();

    expect(el.scrollTop).toBe(1900);
    expect(awayFromBottom.value).toBe(true);
  });

  it('holds the anchor across a rotation for a reader who armed nothing', () => {
    const el = makeTranscript(PORTRAIT);
    const { onScroll, onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    el.scrollTop = el.liveEdge();
    onScroll();

    el.rotate(800, 400);
    onResize();
    el.rotate(400, 800);
    onResize();

    expect(el.scrollTop).toBe(2600);
    expect(el.scrollTop).not.toBe(el.liveEdge());
    expect(awayFromBottom.value).toBe(true);
  });

  it('re-anchors on every step of a drag, not just the first', () => {
    const el = makeTranscript(SIX_TURNS);
    const { onScroll, onResize } = makeScrollObservers(el);

    el.scrollTop = 1900;
    onScroll();
    onResize();

    for (const width of [700, 600, 500, 400]) {
      el.setWidth(width);
      onResize();
      expect(el.turnTop(3)).toBeCloseTo(-100, 6);
    }
  });
});

describe('a rotation keeps the live edge for a reader who asked for it', () => {
  function armedAtTheEdge() {
    const el = makeTranscript(PORTRAIT);
    const observers = makeScrollObservers(el);
    setActiveScrollElement(el);
    el.scrollTop = el.liveEdge();
    observers.onScroll();
    setFollowLiveEdge(true);
    expect(followingLiveEdge.value).toBe(true);
    return { el, ...observers };
  }

  it('brings the reader back to the edge after a rotation and back, on an idle thread', () => {
    const { el, onResize } = armedAtTheEdge();

    el.rotate(800, 400);
    onResize();
    expect(el.scrollTop).toBe(1400);
    expect(el.scrollTop).toBe(el.liveEdge());

    el.rotate(400, 800);
    onResize();

    expect(el.scrollTop).toBe(2800);
    expect(el.scrollTop).toBe(el.liveEdge());
    expect(awayFromBottom.value).toBe(false);
    expect(followingLiveEdge.value).toBe(true);
  });

  it('keeps the edge when only the HEIGHT changes, which re-wraps nothing', () => {
    const { el, onResize } = armedAtTheEdge();

    el.rotate(400, 500);
    onResize();

    expect(el.scrollTop).toBe(3100);
    expect(el.scrollTop).toBe(el.liveEdge());
    expect(awayFromBottom.value).toBe(false);
  });

  it('stands down while a deep-link is still resolving, and keeps the ride', () => {
    const restoreDom = installDeepLinkStubs();
    vi.useFakeTimers();
    try {
      const { el, onResize } = armedAtTheEdge();
      scrollToEventAndPulse('never-renders');
      expect(hasPendingEventScroll()).toBe(true);

      el.rotate(800, 400);
      onResize();
      el.rotate(400, 800);
      onResize();

      expect(el.scrollTop).toBe(2600);
      expect(el.scrollTop).not.toBe(el.liveEdge());
      expect(followingLiveEdge.value).toBe(true);
    } finally {
      clearPendingEventScroll();
      vi.runOnlyPendingTimers();
      vi.useRealTimers();
      restoreDom();
    }
  });

  it('answers the same for content growth, because it is the same reader', () => {
    const { el, onResize } = armedAtTheEdge();

    el.growLastTurn(400);
    onResize();

    expect(el.scrollTop).toBe(3200);
    expect(el.scrollTop).toBe(el.liveEdge());
    expect(awayFromBottom.value).toBe(false);
    expect(followingLiveEdge.value).toBe(true);
  });

  it('ends the ride for a reader who scrolled off the edge, and growth leaves them there', () => {
    const { el, onScroll, onResize } = armedAtTheEdge();

    readerGestureForTest(el);
    el.scrollTop = 1000;
    onScroll();
    readerGestureForTest(null, false);

    el.growLastTurn(400);
    onResize();

    expect(el.scrollTop).toBe(1000);
    expect(awayFromBottom.value).toBe(true);
    expect(followingLiveEdge.value).toBe(false);
  });

  it('stands down for growth under an unresolved deep link too, and keeps the ride', () => {
    const restoreDom = installDeepLinkStubs();
    vi.useFakeTimers();
    try {
      const { el, onResize } = armedAtTheEdge();
      scrollToEventAndPulse('never-renders');
      expect(hasPendingEventScroll()).toBe(true);

      el.growLastTurn(400);
      onResize();

      expect(el.scrollTop).toBe(2800);
      expect(el.scrollTop).not.toBe(el.liveEdge());
      expect(followingLiveEdge.value).toBe(true);
    } finally {
      clearPendingEventScroll();
      vi.runOnlyPendingTimers();
      vi.useRealTimers();
      restoreDom();
    }
  });
});
