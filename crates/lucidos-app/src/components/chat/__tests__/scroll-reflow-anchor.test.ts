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
  followingLiveEdge,
  hasPendingEventScroll,
  makeScrollObservers,
  notAtTop,
  readerGestureForTest,
  scrollToEventAndPulse,
  scrolledFromTop,
  setActiveScrollElement,
  setFollowLiveEdge,
  setThreadLive,
  stopFollowingBottom,
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
  let clientHeight = opts.clientHeight;
  let scrollTop = 0;

  const heightOf = (i: number) => (turns[i] * baseWidth) / width;
  const docTop = (i: number) => {
    let sum = 0;
    for (let n = 0; n < i; n++) sum += heightOf(n);
    return sum;
  };

  const el: any = {
    parentElement: null,
    children: turns.map((_, i) => ({
      isConnected: true,
      getBoundingClientRect: () => ({ top: docTop(i) - scrollTop, height: heightOf(i) }),
    })),
    get clientWidth() { return width; },
    get clientHeight() { return clientHeight; },
    get scrollHeight() { return docTop(turns.length); },
    get scrollTop() { return scrollTop; },
    set scrollTop(next: number) {
      const max = Math.max(0, el.scrollHeight - el.clientHeight);
      scrollTop = Math.max(0, Math.min(next, max));
    },
    getBoundingClientRect: () => ({ top: 0, left: 0, width, height: clientHeight }),

    /** Resize the pane: everything re-wraps, and a shorter transcript clamps. */
    setWidth(next: number) { width = next; el.scrollTop = scrollTop; },
    /** Rotate the device: BOTH dimensions of the box change at once, and the
     *  new width re-wraps every turn. */
    rotate(nextWidth: number, nextHeight: number) {
      width = nextWidth;
      clientHeight = nextHeight;
      el.scrollTop = scrollTop;
    },
    /** The offset the live edge sits at, for the current geometry. */
    liveEdge() { return Math.max(0, el.scrollHeight - el.clientHeight); },
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

/** A phone-shaped transcript: six 600px turns in a 400px-wide, 800px-tall
 *  viewport (3600px of transcript, live edge at 2800). Rotating to a 800px-wide,
 *  400px-tall landscape halves every turn to 1800px total, live edge at 1400. */
const PORTRAIT = { turns: [600, 600, 600, 600, 600, 600], width: 400, clientHeight: 800 };

beforeEach(() => {
  awayFromBottom.value = false;
  notAtTop.value = false;
  scrolledFromTop.value = false;
  stopFollowingBottom();
  setThreadLive(false);
  readerGestureForTest(null, false);
  setActiveScrollElement(null);
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

  it('holds the anchor across a rotation for a reader who armed nothing', () => {
    // The unarmed half of the rotation rule, and the same answer the pane-width
    // case above gives: a position is not a request, so being at the bottom when
    // the phone turned buys nothing. The reader keeps the content they were on
    // and the chevron offers them the ride down.
    const el = makeTranscript(PORTRAIT);
    const { onScroll, onResize } = makeScrollObservers(el);
    setActiveScrollElement(el);

    el.scrollTop = el.liveEdge(); // 2800
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

    // A divider drag delivers one resize per pointer frame.
    for (const width of [700, 600, 500, 400]) {
      el.setWidth(width);
      onResize();
      expect(el.turnTop(3)).toBeCloseTo(-100, 6);
    }
  });
});

/** Rotating the phone changes BOTH dimensions of the transcript's box, and the
 *  new width re-wraps every line. For a reader on the live edge the two things
 *  the anchor could preserve ("the child at the top of my viewport stays there"
 *  and "the content still ends at the bottom of my screen") are one statement
 *  before the rotation and two different ones after it. A reader who ASKED to be
 *  kept at the bottom gets the second; everyone else keeps the child anchor.
 *
 *  Reported 2026-08-13: rotating and back again left the transcript short of the
 *  edge with the toggle lit, because the child anchor was the only answer and
 *  the follow's own write stands down on an idle thread. */
describe('a rotation keeps the live edge for a reader who asked for it', () => {
  /** Park at the live edge and press the toggle there, so arming writes nothing
   *  and runs no tween. Leaves the thread IDLE, which is the case the growth
   *  branch declines and this one must not. */
  function armedAtTheEdge() {
    const el = makeTranscript(PORTRAIT);
    const observers = makeScrollObservers(el);
    setActiveScrollElement(el);
    el.scrollTop = el.liveEdge(); // 2800
    observers.onScroll();
    setFollowLiveEdge(true);
    expect(followingLiveEdge.value).toBe(true);
    return { el, ...observers };
  }

  it('brings the reader back to the edge after a rotation and back, on an idle thread', () => {
    const { el, onResize } = armedAtTheEdge();

    // Landscape: every turn halves (1800px of transcript), and the browser's own
    // clamp happens to land on the new edge.
    el.rotate(800, 400);
    onResize();
    expect(el.scrollTop).toBe(1400);
    expect(el.scrollTop).toBe(el.liveEdge());

    // Back to portrait: the turns double again and nothing clamps, so this is
    // the direction that discriminates. The child anchor would leave them at
    // 2600, which is where the unarmed reader above ends up.
    el.rotate(400, 800);
    onResize();

    expect(el.scrollTop).toBe(2800);
    expect(el.scrollTop).toBe(el.liveEdge());
    expect(awayFromBottom.value).toBe(false);
    expect(followingLiveEdge.value).toBe(true);
  });

  it('leaves an armed reader who is parked in history exactly where they are', () => {
    // The second term of the branch. An idle thread is one an armed reader may
    // browse freely without losing the ride (`followIsCarrying`), so the lit
    // toggle must not become a licence to yank them to the bottom the moment the
    // phone turns.
    const { el, onScroll, onResize } = armedAtTheEdge();

    // Their OWN hand, because that is the only way an armed reader gets parked
    // in history and keeps the ride. A scroll with no gesture behind it is the
    // platform's, and the follow puts them straight back on the edge for one
    // (`keepTheLiveEdge`); browsing is what this case is about.
    readerGestureForTest(el);
    el.scrollTop = 1000;
    onScroll();
    expect(followingLiveEdge.value).toBe(true); // browsing an idle thread keeps it
    readerGestureForTest(null, false);          // and the coast lapses before the rotation

    el.rotate(800, 400);
    onResize();

    expect(el.turnTop(1)).toBe(-400); // held on the same content
    expect(el.scrollTop).toBe(700);
    expect(el.scrollTop).not.toBe(el.liveEdge());
  });

  it('keeps the edge when only the HEIGHT changes, which re-wraps nothing', () => {
    // The keyboard and the composer take height from the transcript without
    // touching its width, and strand a reader on the edge in exactly the same
    // way. There is no reflow to correct here, which is why the branch is gated
    // on the box changing rather than on a re-wrap.
    const { el, onResize } = armedAtTheEdge();

    el.rotate(400, 500); // the soft keyboard takes 300px
    onResize();

    expect(el.scrollTop).toBe(3100);
    expect(el.scrollTop).toBe(el.liveEdge());
    expect(awayFromBottom.value).toBe(false);
  });

  it('stands down while a deep-link is still resolving, and keeps the ride', () => {
    // A link owns the position for the WHOLE resolve window, not just from its
    // landing, and most of that window has no tween in it: the thread is still
    // loading and the target has not rendered. The reader is meanwhile wherever
    // the outgoing thread left the shared container, so writing the live edge
    // over a link in flight is both the wrong place and a guess. The ride
    // survives it, because standing down is about the WRITE.
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

      expect(el.scrollTop).toBe(2600); // the anchor's answer, as for an unarmed reader
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
    // Same reader, same lit toggle, same position on the edge, and this time the
    // box is untouched and the CONTENT grows. INVERTED on 2026-08-13, having
    // asserted the opposite for a day: a rotation and an arriving question card
    // are two ways of sliding the live edge out from under a rider who has not
    // moved, and answering them differently painted the card with its options
    // below the fold and the chevron up under a lit toggle.
    //
    // The stand-down this replaced came from 2026-08-11, which is a real report
    // about a real reader: one who had SCROLLED UP to re-read a finished reply
    // and was hauled back down by the next markdown reflow. That reader is still
    // left alone, by the position term rather than by the liveness one (see the
    // test below, and the `an IDLE thread moves an armed reader nowhere` block in
    // `scroll-follow-the-live-edge.test.ts`).
    const { el, onResize } = armedAtTheEdge();

    el.growLastTurn(400);
    onResize();

    expect(el.scrollTop).toBe(3200);
    expect(el.scrollTop).toBe(el.liveEdge());
    expect(awayFromBottom.value).toBe(false);
    expect(followingLiveEdge.value).toBe(true);
  });

  it('leaves an armed reader who scrolled off the edge alone when content grows', () => {
    // The other side of the inverted test above, kept beside it so the two read
    // as one rule. What decides is WHERE the reader is, never what kind of resize
    // it was: a rider is kept on the edge, and a browser is left where they
    // parked. Neither loses the ride.
    const { el, onScroll, onResize } = armedAtTheEdge();

    // Their OWN hand, for the same reason as the rotation case above: a scroll
    // with no gesture behind it is the platform's, and `keepTheLiveEdge` puts
    // them straight back on the edge for one.
    readerGestureForTest(el);
    el.scrollTop = 1000;
    onScroll();
    readerGestureForTest(null, false); // the coast lapses before the growth

    el.growLastTurn(400);
    onResize();

    expect(el.scrollTop).toBe(1000);
    expect(awayFromBottom.value).toBe(true);
    expect(followingLiveEdge.value).toBe(true);
  });

  it('stands down for growth under an unresolved deep link too, and keeps the ride', () => {
    // The deep-link term is inside `keepTheLiveEdge` rather than at each call
    // site, so it has to hold for GROWTH exactly as it does for the rotation
    // above. This is the ordinary shape of a notification tap: the thread is
    // still rendering, so the transcript grows for a second while the link's
    // target has not appeared yet, and there is no tween in that window to stand
    // down for. A live-edge write there would carry the reader past the event
    // they tapped before the link ever got to place them.
    const restoreDom = installDeepLinkStubs();
    vi.useFakeTimers();
    try {
      const { el, onResize } = armedAtTheEdge();
      scrollToEventAndPulse('never-renders');
      expect(hasPendingEventScroll()).toBe(true);

      el.growLastTurn(400);
      onResize();

      expect(el.scrollTop).toBe(2800);
      expect(el.scrollTop).not.toBe(el.liveEdge()); // 3200
      expect(followingLiveEdge.value).toBe(true);
    } finally {
      clearPendingEventScroll();
      vi.runOnlyPendingTimers();
      vi.useRealTimers();
      restoreDom();
    }
  });
});
