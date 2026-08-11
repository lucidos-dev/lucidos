import { describe, it, expect, beforeEach, afterEach } from 'vitest';

// Stub HTMLElement before importing modules that reference it (matches the other
// scroll-*.test.ts headers — the test env is minimal node, not jsdom).
if (typeof globalThis.HTMLElement === 'undefined') {
  (globalThis as any).HTMLElement = class HTMLElement {};
}
if (typeof globalThis.requestAnimationFrame === 'undefined') {
  (globalThis as any).requestAnimationFrame = (cb: any) => { cb(0); return 0; };
  (globalThis as any).cancelAnimationFrame = () => {};
}

import {
  stepThreadTurn,
  setActiveScrollElement,
  awayFromBottom,
  followingLiveEdge,
  setFollowLiveEdge,
  setThreadLive,
  stopFollowingBottom,
} from '../scrollState';

// ---------------------------------------------------------------------------
// stepThreadTurn — chevron reconciliation on the landing position
// ---------------------------------------------------------------------------
// The ⌘↑/⌘↓ turn-nav used to force awayFromBottom=true on every jump. When a
// jump lands at the bottom while ALREADY at the bottom (press ⌘↓ onto the last
// turn from the bottom), the clamped scroll target equals the current position,
// so nothing scrolls, no scroll event fires, and onScroll never reconciles the
// forced value — leaving the down chevron stuck on ("appears the second time you
// click down arrow"). stepThreadTurn now sets the position signals from the
// computed landing target instead of hardcoding them, so the chevron is honest
// whether or not the container actually moves.
//
// These run through the reduced-motion path (below) so the jump is a single
// synchronous scrollTop write with no rAF animation to await — the signal logic
// under test is set before the reduced-motion branch, so it is identical on the
// animated path.

/** A turn (`.chat-exchange`) whose viewport-relative top tracks the container's
 *  live scrollTop, exactly like the real DOM: content-top minus scrollTop. */
function makeTurn(contentTop: number, el: { scrollTop: number }, height = 60) {
  return {
    parentElement: null,
    classList: { add() {}, remove() {} },
    getBoundingClientRect: () => ({ top: contentTop - el.scrollTop, width: 400, height }),
  };
}

/** A mock .thread-content scroll container. `turns` are the visible chat
 *  exchanges it reports via querySelectorAll. containerTop is fixed at 0. */
function makeContainer(opts: { scrollTop: number; scrollHeight: number; clientHeight: number }) {
  const turns: any[] = [];
  const el: any = {
    scrollTop: opts.scrollTop,
    scrollHeight: opts.scrollHeight,
    clientHeight: opts.clientHeight,
    parentElement: null,
    focus() {},
    getBoundingClientRect: () => ({ top: 0, width: 400, height: opts.clientHeight }),
    querySelectorAll: (_sel: string) => turns,
  };
  return { el, turns };
}

describe('stepThreadTurn — down chevron on the last turn', () => {
  let restoreMatchMedia: () => void;
  let restoreGCS: () => void;

  beforeEach(() => {
    awayFromBottom.value = false;
    // Force reduced motion so the jump is a synchronous scrollTop write (no rAF
    // animation loop to drive/await in the node test env).
    const origMM = globalThis.matchMedia;
    (globalThis as any).matchMedia = () => ({ matches: true, addEventListener() {}, removeEventListener() {} });
    restoreMatchMedia = () => { (globalThis as any).matchMedia = origMM; };
    // Deterministic clearance: turnNavClearancePx reads scroll-margin-top; pin it
    // to 8px so the landing gap is stable across environments.
    const origGCS = (globalThis as any).getComputedStyle;
    (globalThis as any).getComputedStyle = () => ({ scrollMarginTop: '8px', display: 'block' });
    restoreGCS = () => { (globalThis as any).getComputedStyle = origGCS; };
  });

  afterEach(() => {
    setActiveScrollElement(null);
    restoreMatchMedia();
    restoreGCS();
  });

  it('does NOT show the chevron when a down-jump lands at the bottom (the regression)', () => {
    // At the bottom (scrollTop 500 of maxScroll 500). The last turn (content top
    // 920, short) is fully on screen; pressing ⌘↓ picks it but its landing target
    // clamps to the bottom, so the container can't move — the old code left
    // awayFromBottom forced true here.
    const { el, turns } = makeContainer({ scrollTop: 500, scrollHeight: 1000, clientHeight: 500 });
    turns.push(makeTurn(0, el), makeTurn(300, el), makeTurn(920, el));
    setActiveScrollElement(el);

    stepThreadTurn(1);

    expect(awayFromBottom.value).toBe(false); // chevron hidden: we are at the bottom
  });

  it('keeps the chevron when a down-jump lands mid-thread (content still below)', () => {
    // Jump from the top to turn B (content top 800) with a long thread below —
    // the landing target (792) is far above maxScroll (1500), so the chevron
    // must stay on and the user is parked mid-thread.
    const { el, turns } = makeContainer({ scrollTop: 0, scrollHeight: 2000, clientHeight: 500 });
    turns.push(makeTurn(0, el), makeTurn(800, el), makeTurn(1600, el));
    setActiveScrollElement(el);

    stepThreadTurn(1);

    expect(awayFromBottom.value).toBe(true); // chevron shown: there is content below
  });

  it('reaching the last turn (landed one turn above) lands at the bottom with no chevron', () => {
    // Long thread, last turn near the end (content top 1520 of maxScroll 1500).
    // Sitting on the second-to-last turn (scrollTop 700) the down-jump picks the
    // last turn; its landing target (1512) is at/beyond the bottom, so even though
    // the container does move a little the reconciled chevron ends hidden.
    const { el, turns } = makeContainer({ scrollTop: 700, scrollHeight: 2000, clientHeight: 500 });
    turns.push(makeTurn(0, el), makeTurn(700, el), makeTurn(1520, el));
    setActiveScrollElement(el);

    stepThreadTurn(1);

    expect(awayFromBottom.value).toBe(false);
  });
});

/**
 * **Turn stepping ends a standing follow, and says so itself.**
 *
 * A scroll only speaks for the reader when a gesture is behind it (see
 * scrollState's "Was this scroll the reader's own GESTURE?"), and a keyboard
 * chord is not one: `stepThreadTurn`'s write used to retire the ride only
 * because it landed somewhere the position test did not recognise, which was
 * always an accident rather than a reading of intent.
 *
 * It reuses the chevron's own landing verdict, so the two cannot disagree: a
 * step that parks the reader mid-thread ends the ride, and one that lands on
 * the last turn does not, because the clamped live edge is where the ride was
 * taking them anyway.
 */
describe('stepThreadTurn and the standing follow', () => {
  let restoreMatchMedia: () => void;
  let restoreGCS: () => void;

  beforeEach(() => {
    awayFromBottom.value = false;
    stopFollowingBottom();
    setThreadLive(true);
    const origMM = globalThis.matchMedia;
    (globalThis as any).matchMedia = () => ({ matches: true, addEventListener() {}, removeEventListener() {} });
    restoreMatchMedia = () => { (globalThis as any).matchMedia = origMM; };
    const origGCS = (globalThis as any).getComputedStyle;
    (globalThis as any).getComputedStyle = () => ({ scrollMarginTop: '8px', display: 'block' });
    restoreGCS = () => { (globalThis as any).getComputedStyle = origGCS; };
  });

  afterEach(() => {
    stopFollowingBottom();
    setActiveScrollElement(null);
    restoreMatchMedia();
    restoreGCS();
  });

  /** An armed reader on a transcript long enough to step around in. Arming
   *  takes them to the live edge, which is where a riding reader is by
   *  definition, so the step that has anywhere to go is BACKWARDS. */
  function ridingLongThread() {
    const { el, turns } = makeContainer({ scrollTop: 0, scrollHeight: 2000, clientHeight: 500 });
    turns.push(makeTurn(0, el), makeTurn(800, el), makeTurn(1600, el));
    setActiveScrollElement(el);
    setFollowLiveEdge(true);
    expect(followingLiveEdge.value).toBe(true);
    return el;
  }

  it('ends the ride on a step that parks the reader mid-thread', () => {
    ridingLongThread();

    stepThreadTurn(-1);

    expect(awayFromBottom.value).toBe(true);   // parked, content still below
    expect(followingLiveEdge.value).toBe(false);
  });

  it('keeps the ride on a step that lands at the live edge', () => {
    // The clamped last turn is where the ride was taking them, so nothing was
    // taken away and the toggle stays lit.
    const { el, turns } = makeContainer({ scrollTop: 700, scrollHeight: 2000, clientHeight: 500 });
    turns.push(makeTurn(0, el), makeTurn(700, el), makeTurn(1520, el));
    setActiveScrollElement(el);
    setFollowLiveEdge(true);
    expect(followingLiveEdge.value).toBe(true);

    stepThreadTurn(1);

    expect(awayFromBottom.value).toBe(false);
    expect(followingLiveEdge.value).toBe(true);
  });

  it('keeps the ride when the thread is IDLE, whatever the step does', () => {
    // Same rule as the scroll disarm: stepping back through a thread nothing is
    // writing to is browsing, and the reader's next submit should still carry
    // them to the live edge.
    ridingLongThread();
    setThreadLive(false);

    stepThreadTurn(-1);

    expect(awayFromBottom.value).toBe(true);
    expect(followingLiveEdge.value).toBe(true);
  });
});
