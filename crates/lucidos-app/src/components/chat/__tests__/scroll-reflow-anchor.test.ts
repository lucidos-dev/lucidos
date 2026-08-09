// Resizing a pane changes the transcript's WIDTH, and every wrapped line
// re-wraps: the content above the viewport changes height, so the same scrollTop
// shows a different part of the thread. Narrowing the thread pane used to carry
// the reader up into older turns. makeScrollObservers' resize handler anchors
// against that; these tests pin the anchoring.
//
// The fake transcript below models the one thing that matters here: a turn's
// height scales inversely with the container width, the way wrapped text does.
import { describe, it, expect, beforeEach, vi } from 'vitest';

// Stub HTMLElement before importing modules that reference it (this suite runs
// in the node environment, like the other scroll-*.test.ts files).
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
  hasPendingEventScroll,
  makeScrollObservers,
  notAtTop,
  scrollToEventAndPulse,
  scrolledFromTop,
} from '../scrollState';

/** Enough DOM for `scrollToEventAndPulse` to arm a deep-link claim and then wait
 *  for a target that never renders (the async branch, which is the one that
 *  holds the claim while a thread loads). */
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

/** A `.thread-content` stand-in whose turns re-wrap when the width changes.
 *  Heights scale by `baseWidth / width`, so halving the width doubles every
 *  turn. scrollTop clamps like a real container, both on write and whenever a
 *  layout change shortens the content (a widening pane does exactly that, and
 *  the clamp it forces is the case a delta-based anchor gets wrong). The
 *  container's own viewport top is 0, so a turn's `getBoundingClientRect().top`
 *  IS its offset from the viewport top. */
function makeTranscript(opts: { turns: number[]; width: number; clientHeight: number }) {
  const baseWidth = opts.width;
  const turns = [...opts.turns];
  let width = opts.width;
  let scrollTop = 0;

  const heightOf = (i: number) => (turns[i] * baseWidth) / width;
  const docTop = (i: number) => {
    let sum = 0;
    for (let n = 0; n < i; n++) sum += heightOf(n);
    return sum;
  };

  const el: any = {
    parentElement: null,
    clientHeight: opts.clientHeight,
    children: turns.map((_, i) => ({
      isConnected: true,
      getBoundingClientRect: () => ({ top: docTop(i) - scrollTop, height: heightOf(i) }),
    })),
    get clientWidth() { return width; },
    get scrollHeight() { return docTop(turns.length); },
    get scrollTop() { return scrollTop; },
    set scrollTop(next: number) {
      const max = Math.max(0, el.scrollHeight - el.clientHeight);
      scrollTop = Math.max(0, Math.min(next, max));
    },
    getBoundingClientRect: () => ({ top: 0, left: 0, width, height: opts.clientHeight }),

    /** Resize the pane: everything re-wraps, and a shorter transcript clamps. */
    setWidth(next: number) { width = next; el.scrollTop = scrollTop; },
    /** Streaming growth: the newest turn gets taller, the width does not change. */
    growLastTurn(px: number) { turns[turns.length - 1] += px; el.scrollTop = scrollTop; },
    /** A turn's top relative to the viewport top (negative once scrolled past). */
    turnTop(i: number) { return docTop(i) - scrollTop; },
  };
  return el;
}

/** Six 600px turns in an 800px-wide, 600px-tall viewport: 3600px of transcript.
 *  Halving the width to 400 doubles every turn to 7200px total. */
const SIX_TURNS = { turns: [600, 600, 600, 600, 600, 600], width: 800, clientHeight: 600 };

beforeEach(() => {
  awayFromBottom.value = false;
  notAtTop.value = false;
  scrolledFromTop.value = false;
});

describe('transcript reflow anchoring across a pane-width change', () => {
  it('keeps the turn the reader is on in place when the pane narrows', () => {
    const el = makeTranscript(SIX_TURNS);
    const { onScroll, onResize } = makeScrollObservers(el);

    // Park mid-history with turn 3's top 100px above the viewport top.
    el.scrollTop = 1900;
    onScroll();
    onResize(); // layout settles: the anchor snapshot is taken here
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

    // Widening halves every turn: 3600px of transcript becomes 1800px, whose
    // maximum offset (1200) is below where the reader was, so the browser
    // clamps them there before the observer ever runs. The correction reads two
    // measured positions rather than the scrollTop delta, so the clamp does not
    // masquerade as the reader having scrolled up 700px.
    el.setWidth(800);
    expect(el.scrollTop).toBe(1200);
    onResize();

    expect(el.turnTop(3)).toBe(-100);
    expect(el.scrollTop).toBe(1000);
  });

  it('anchors a reader sitting at the bottom too, rather than re-pinning them', () => {
    // The correction used to branch here: a reader within 80px of the bottom was
    // put back ON the new bottom (6600) instead of held on their content. That
    // is a bottom-pin wearing anchor preservation's clothes, and it fired on
    // someone who had deliberately scrolled 79px up. Everyone gets the anchor,
    // so the last turn's top stays exactly where the reader had it and the
    // chevron comes up for the content that the re-wrap pushed below the fold.
    const el = makeTranscript(SIX_TURNS);
    const { onScroll, onResize } = makeScrollObservers(el);

    el.scrollTop = 3000; // the bottom (3600 of content, 600 of viewport)
    onScroll();
    expect(awayFromBottom.value).toBe(false);
    onResize();
    expect(el.turnTop(5)).toBe(0); // the last turn starts at the viewport top

    el.setWidth(400);
    onResize();

    expect(el.turnTop(5)).toBe(0); // still on it, not dragged to the new bottom
    expect(el.scrollTop).toBe(6000);
    expect(awayFromBottom.value).toBe(true); // the re-wrap put content below them
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

      // A deep-link owns the viewport for the whole resolve window, and the
      // anchor correction must not fight the landing it is about to make.
      scrollToEventAndPulse('never-renders');
      expect(hasPendingEventScroll()).toBe(true);

      // revealThreadPane's 300ms re-expansion fires the observer mid-load.
      el.setWidth(400);
      onResize();

      expect(el.scrollTop).not.toBe(6600); // NOT slammed to the new bottom
      expect(el.turnTop(3)).toBe(-100);    // held on the anchor instead
    } finally {
      // Drop the claim before running the deadline out, so its give-up branch
      // sees a superseded claim and reports nothing.
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
    onResize(); // anchored on turn 3

    // Scrolling fires no resize, so the scroll handler is the only thing that
    // can move the anchor onto the turn now at the viewport top.
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

    // A token lands: the last turn grows, the width is untouched. The reader
    // stays exactly where they are, and the chevron comes up.
    el.growLastTurn(400);
    onResize();

    expect(el.scrollTop).toBe(1900);
    expect(awayFromBottom.value).toBe(true);
  });

  it('re-anchors on every step of a drag, not just the first', () => {
    const el = makeTranscript(SIX_TURNS);
    const { onScroll, onResize } = makeScrollObservers(el);

    el.scrollTop = 1900;
    onScroll();
    onResize();

    // A divider drag delivers one resize per pointer frame.
    for (const width of [700, 600, 500, 400]) {
      el.setWidth(width);
      onResize();
      expect(el.turnTop(3)).toBeCloseTo(-100, 6);
    }
  });
});
