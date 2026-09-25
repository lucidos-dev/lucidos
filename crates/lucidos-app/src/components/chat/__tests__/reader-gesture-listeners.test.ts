import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

if (typeof (globalThis as any).HTMLElement === 'undefined') {
  (globalThis as any).HTMLElement = class {};
}

import {
  followingLiveEdge,
  isScrollbarHeld,
  makeScrollObservers,
  onScrollbarReleased,
  readerGestureForTest,
  scrollbarReleased,
  setActiveScrollElement,
  setFollowLiveEdge,
  setTranscriptLive,
  stopFollowingBottom,
} from '../scrollState';

/**
 * **What the listeners themselves make of each input.**
 *
 * The suite next door (`scroll-follow-the-live-edge.test.ts`) drives the
 * DECISION through `readerGestureForTest`, which states the fact the listeners
 * record without going near them. That is the right shape for asking what the
 * follow does with a gesture, and it leaves the other half unasked: whether a
 * given input produces one. The distinction is not academic. Every bug this
 * mechanism has had so far lived on this side of the line, in which event
 * counts as the reader:
 *
 *   - arming on `pointerdown` put a window over every press inside the
 *     transcript, and a press is how the reader answers a question or grants a
 *     permission (both of which must KEEP the follow);
 *   - reading `offsetX` off a bubbled press measured the wrong box, so an
 *     ordinary tap low in a tall turn read as a press in the scrollbar gutter;
 *   - `Cmd+ArrowDown` is turn stepping, not a scroll key, and stamping it
 *     defeated the one case `stepThreadTurn` keeps the ride for.
 *
 * So this file drives real events at the real listeners, through a container
 * double that records what was registered on it.
 */
describe('what the reader-gesture listeners count as a scroll', () => {
  /** A container that records its listeners, plus `window`'s, so a test can
   *  fire either and the teardown can be checked for both. */
  function makeContainer() {
    const own: Record<string, Function[]> = {};
    const el: any = {
      parentElement: null,
      children: [],
      clientWidth: 800,
      clientHeight: 500,
      scrollHeight: 3000,
      _scrollTop: 2500,
      get scrollTop() { return this._scrollTop; },
      set scrollTop(v: number) {
        this._scrollTop = Math.min(Math.max(0, v), Math.max(0, this.scrollHeight - this.clientHeight));
      },
      getBoundingClientRect: () => ({ width: 800, height: 500, top: 0, bottom: 500, left: 0, right: 800 }),
      querySelectorAll: () => [],
      listeners: own,
      addEventListener(type: string, fn: Function) { (own[type] ??= []).push(fn); },
      removeEventListener(type: string, fn: Function) {
        own[type] = (own[type] ?? []).filter(f => f !== fn);
      },
      /** Fire an event at the container's own listeners. `target` defaults to
       *  the container, which is what a press on its scrollbar reports. */
      fire(type: string, event: Record<string, unknown> = {}) {
        for (const fn of own[type] ?? []) fn({ target: el, button: 0, ...event });
      },
    };
    return el;
  }

  let windowListeners: Record<string, Function[]>;
  let realWindow: any;

  beforeEach(() => {
    windowListeners = {};
    realWindow = (globalThis as any).window;
    (globalThis as any).window = {
      addEventListener(type: string, fn: Function) { (windowListeners[type] ??= []).push(fn); },
      removeEventListener(type: string, fn: Function) {
        windowListeners[type] = (windowListeners[type] ?? []).filter(f => f !== fn);
      },
    };
    stopFollowingBottom();
    // A live thread, where a reader's scroll turns the follow off. A waiting
    // thread parks it instead (`scroll-follow-the-live-edge.test.ts`).
    setTranscriptLive(true);
    setActiveScrollElement(null);
    readerGestureForTest(null, false);
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.clearAllTimers();
    vi.useRealTimers();
    (globalThis as any).window = realWindow;
    stopFollowingBottom();
    setActiveScrollElement(null);
    readerGestureForTest(null, false);
  });

  /** An armed reader, with the listeners really attached. */
  function riding() {
    const el = makeContainer();
    const observers = makeScrollObservers(el);
    setActiveScrollElement(el);
    setFollowLiveEdge(true);
    vi.advanceTimersByTime(1500);
    expect(followingLiveEdge.value).toBe(true);
    return { el, ...observers };
  }

  /** Move the container the way the platform does: no input event at all. */
  function platformScrollsTo(el: any, top: number, onScroll: () => void) {
    el.scrollTop = top;
    onScroll();
  }

  /** A press in the scrollbar gutter, past the client box. */
  function gutterPress(el: any) {
    el.fire('pointerdown', { offsetX: el.clientWidth + 6, offsetY: 200 });
  }

  /** Fire an event at `window`'s listeners, where the releases live. */
  function fireWindow(type: string, event: Record<string, unknown> = {}) {
    for (const fn of windowListeners[type] ?? []) fn(event);
  }

  /** `riding()`, plus the glide's own trailing scroll event, so the cases that
   *  ask what the transcript then DOES start from a settled ride. */
  function ridingAndAnchored() {
    const r = riding();
    r.onScroll();
    return r;
  }

  it('attaches to the container, and the releases to window', () => {
    const el = makeContainer();
    const { detachGestures } = makeScrollObservers(el);

    expect(Object.keys(el.listeners).sort()).toEqual(
      ['focusin', 'keydown', 'pointerdown', 'pointermove', 'touchmove', 'wheel'],
    );
    // The release goes on `window`: a drag that ends with the pointer outside
    // the transcript would otherwise leave the press recorded forever.
    expect(Object.keys(windowListeners).sort()).toEqual(
      ['blur', 'pointercancel', 'pointerup', 'touchcancel', 'touchend'],
    );

    detachGestures();
    for (const fns of Object.values(el.listeners)) expect(fns).toEqual([]);
    for (const fns of Object.values(windowListeners)) expect(fns).toEqual([]);
  });

  it('counts a wheel notch', () => {
    const { el, onScroll } = riding();
    el.fire('wheel');
    platformScrollsTo(el, 900, onScroll);
    expect(followingLiveEdge.value).toBe(false);
  });

  it('counts a finger travelling', () => {
    const { el, onScroll } = riding();
    el.fire('touchmove');
    platformScrollsTo(el, 900, onScroll);
    expect(followingLiveEdge.value).toBe(false);
  });

  it('counts a scroll key', () => {
    const { el, onScroll } = riding();
    el.fire('keydown', { key: 'PageDown' });
    platformScrollsTo(el, 900, onScroll);
    expect(followingLiveEdge.value).toBe(false);
  });

  it('does NOT count a chord, which is a shortcut rather than a scroll key', () => {
    // Cmd+Arrow is turn stepping. Stamping it would retire the ride from
    // `onScroll` mid-glide, defeating the one case `stepThreadTurn` keeps it
    // for: a step onto the last turn, which lands at the live edge anyway.
    const { el, onScroll } = riding();
    el.fire('keydown', { key: 'ArrowDown', metaKey: true });
    platformScrollsTo(el, 900, onScroll);
    expect(followingLiveEdge.value).toBe(true);
  });

  it('does NOT count a key that scrolls nothing', () => {
    const { el, onScroll } = riding();
    el.fire('keydown', { key: 'a' });
    platformScrollsTo(el, 900, onScroll);
    expect(followingLiveEdge.value).toBe(true);
  });

  it('reads a scroll key on a control INSIDE the transcript as a placement, not the platform', () => {
    // A scroll key the focused control does not consume still scrolls the
    // transcript, because the browser scrolls the nearest scrollable ancestor.
    // The choice-card seeding parks focus on a button INSIDE the transcript. A
    // reader answering a question and then paging back is in exactly this state. The correction must stand down or keyboard scrolling
    // is undone the instant it happens. The reader moved themselves off the
    // edge, so the ride ends with it.
    const { el, onScroll } = ridingAndAnchored();

    el.fire('keydown', { key: 'PageUp', target: { nodeName: 'BUTTON' } });
    platformScrollsTo(el, 900, onScroll);

    expect(el.scrollTop).toBe(900);
    expect(followingLiveEdge.value).toBe(false);
  });

  it('and answers the platform again once that keypress is four frames old', () => {
    // A window, like every other answer to "was that scroll ours". A PageUp four
    // frames ago says nothing about the keyboard adjusting the offset now.
    const { el, onScroll } = ridingAndAnchored();

    el.fire('keydown', { key: 'PageUp', target: { nodeName: 'BUTTON' } });
    vi.advanceTimersByTime(200);
    platformScrollsTo(el, 900, onScroll);

    expect(el.scrollTop).toBe(2500);
    expect(followingLiveEdge.value).toBe(true);
  });

  it('keeps the reader where FOCUS landing inside it took them, and ends the ride', () => {
    // The other way the container scrolls with nobody writing `scrollTop`: the
    // browser reveals a focused control that is off screen, for Tab, Shift+Tab,
    // a screen reader moving the cursor, or any `focus()` without
    // `preventScroll`. It is a PLACEMENT, so the correction must not write them
    // back. Otherwise Tab appears to do nothing, with its control off screen.
    // The reader is off the edge, so the toggle goes off.
    const { el, onScroll, onResize } = ridingAndAnchored();
    const buttonInAnOlderTurn = { nodeName: 'BUTTON' };

    el.fire('focusin', { target: buttonInAnOlderTurn });
    platformScrollsTo(el, 400, onScroll);

    expect(el.scrollTop).toBe(400);
    expect(followingLiveEdge.value).toBe(false);

    el.scrollHeight = 3100;   // and the next growth moves them nowhere
    onResize();
    expect(el.scrollTop).toBe(400);
  });

  it('and answers the platform again once the focus reveal is four frames old', () => {
    // The stamp is a WINDOW, like every other answer to "was that scroll ours":
    // a focus move four frames ago says nothing about a scroll now, and if it
    // did, one Tab would exempt the rest of the thread from the correction.
    const { el, onScroll } = ridingAndAnchored();

    el.fire('focusin', { target: { nodeName: 'BUTTON' } });
    vi.advanceTimersByTime(200);
    platformScrollsTo(el, 400, onScroll);

    expect(el.scrollTop).toBe(2500);   // carried back to the live edge
    expect(followingLiveEdge.value).toBe(true);
  });

  it('counts a scrollbar DRAG for as long as the thumb is held', () => {
    // The press stamps once; a slow haul down the bar can outlast the window,
    // so the moves under it keep the signal fresh.
    const { el, onScroll } = riding();
    gutterPress(el);
    vi.advanceTimersByTime(1000);
    el.fire('pointermove', { buttons: 1 });
    vi.advanceTimersByTime(1000);
    el.fire('pointermove', { buttons: 1 });
    platformScrollsTo(el, 900, onScroll);
    expect(followingLiveEdge.value).toBe(false);
  });

  it('does NOT count a pointer merely crossing the transcript', () => {
    // On a desktop this fires constantly. Without the press gate the signal
    // would be permanently in flight and every platform scroll would retire.
    const { el, onScroll } = riding();
    el.fire('pointermove', { buttons: 0 });
    platformScrollsTo(el, 900, onScroll);
    expect(followingLiveEdge.value).toBe(true);
  });

  it('does NOT count the jitter every real click on a control carries', () => {
    // A press on content never arms the drag path at all, so the sub-pixel
    // movement between pressing a question card's button and releasing it
    // cannot stamp. Recording content presses is what made that reachable.
    const { el, onScroll } = riding();
    const answerButton = { nodeName: 'BUTTON' };

    el.fire('pointerdown', { target: answerButton, offsetX: 40, offsetY: 12 });
    el.fire('pointermove', { buttons: 1 }); // the finger settling on the button
    el.fire('pointermove', { buttons: 1 });
    platformScrollsTo(el, 900, onScroll);   // and the card's reflow

    expect(followingLiveEdge.value).toBe(true);
  });

  it('counts a press in the scrollbar GUTTER, which scrolls with no movement', () => {
    // Clicking the track pages the transcript in one jump, so there is no move
    // to wait for. The gutter is outside the client box, which is what makes
    // this the one press that cannot be a content control.
    const { el, onScroll } = riding();
    gutterPress(el);
    platformScrollsTo(el, 900, onScroll);
    expect(followingLiveEdge.value).toBe(false);
  });

  it('does NOT count an ordinary press, wherever in the transcript it lands', () => {
    // Answering a question, granting a permission, expanding a turn. The
    // `offsetY` case is the one that shipped broken: `offsetX`/`offsetY` are
    // measured from the TARGET's box and `pointerdown` bubbles, so a tap low in
    // a turn taller than the viewport reported an offset past the container's
    // client box and read as the gutter.
    const { el, onScroll } = riding();
    const tallTurn = { nodeName: 'DIV' };

    el.fire('pointerdown', { target: tallTurn, offsetX: 900, offsetY: 4000 });
    platformScrollsTo(el, 900, onScroll);

    expect(followingLiveEdge.value).toBe(true);
  });

  it('forgets a scrollbar press released where it could not see it', () => {
    // A release over a nested iframe, or one that happens while the PWA is
    // backgrounded, never reaches the window listener. The next move with no
    // button held is what clears it, so the press cannot stick and turn an
    // ordinary hover into a drag.
    const { el, onScroll } = riding();

    gutterPress(el);
    el.fire('pointermove', { buttons: 0 }); // the release we never saw
    vi.advanceTimersByTime(2000);           // and the press's own stamp lapses
    el.fire('pointermove', { buttons: 0 }); // now just a hover
    platformScrollsTo(el, 900, onScroll);

    expect(followingLiveEdge.value).toBe(true);
  });

  it('ends the press on a release anywhere, including outside the transcript', () => {
    const { el, onScroll } = riding();

    gutterPress(el);
    fireWindow('pointerup');
    vi.advanceTimersByTime(2000);           // the press's own stamp lapses
    el.fire('pointermove', { buttons: 1 }); // a drag that belongs to something else
    platformScrollsTo(el, 900, onScroll);

    expect(followingLiveEdge.value).toBe(true);
  });

  describe('a held scrollbar, which the window waits out (ADR 0258)', () => {
    // While the thumb is held, Chromium puts its own drag position back and
    // undoes an anchor write. So ThreadView grows the window, and lets history
    // land above the reader, on the RELEASE, which these exports report.
    let released: unknown[];
    let unsubscribe: () => void;
    let realDocument: any;
    beforeEach(() => {
      released = [];
      unsubscribe = onScrollbarReleased((el) => released.push(el));
      realDocument = (globalThis as any).document;
    });
    afterEach(() => {
      unsubscribe();
      (globalThis as any).document = realDocument;
    });

    it('is held from a gutter press until the release, which is announced once', () => {
      const { el } = riding();
      gutterPress(el);
      expect(isScrollbarHeld(el)).toBe(true);

      fireWindow('pointerup');
      expect(isScrollbarHeld(el)).toBe(false);
      expect(released).toEqual([el]);

      fireWindow('pointerup');
      expect(released).toEqual([el]);
    });

    it('is held by a press on the container inside its client box, where an overlay thumb sits', () => {
      const { el } = ridingAndAnchored();
      el.fire('pointerdown', { offsetX: el.clientWidth - 4, offsetY: 200 });
      expect(isScrollbarHeld(el)).toBe(true);
    });

    it('leaves the reader where an overlay thumb drag put them', () => {
      // macOS draws overlay scrollbars, so the press lands inside the client
      // box. Writing the reader back to the live edge here fought Chromium,
      // which puts its own drag position back every frame: the content shook
      // and the thumb would not leave the bottom.
      const { el, onScroll } = ridingAndAnchored();
      el.fire('pointerdown', { offsetX: el.clientWidth - 4, offsetY: 200 });
      platformScrollsTo(el, 400, onScroll);
      expect(el.scrollTop).toBe(400);
      expect(followingLiveEdge.value).toBe(false);
    });

    it('retires the ride on an overlay thumb drag, however slow', () => {
      // Chromium may send no pointer moves while it drives its own thumb, so
      // the hold itself says the reader is scrolling, not a fresh stamp.
      const { el, onScroll } = ridingAndAnchored();
      el.fire('pointerdown', { offsetX: el.clientWidth - 4, offsetY: 200 });
      vi.advanceTimersByTime(3000);
      platformScrollsTo(el, 400, onScroll);
      expect(el.scrollTop).toBe(400);
      expect(followingLiveEdge.value).toBe(false);
    });

    it('answers the platform again once the thumb is released', () => {
      const { el, onScroll } = ridingAndAnchored();
      el.fire('pointerdown', { offsetX: el.clientWidth - 4, offsetY: 200 });
      fireWindow('pointerup');
      platformScrollsTo(el, 400, onScroll);
      expect(el.scrollTop).toBe(2500);
      expect(followingLiveEdge.value).toBe(true);
    });

    it('is never held by a press on content, so no release is announced', () => {
      const { el } = riding();
      el.fire('pointerdown', { target: { nodeName: 'BUTTON' }, offsetX: 40, offsetY: 12 });
      expect(isScrollbarHeld(el)).toBe(false);
      fireWindow('pointerup');
      expect(released).toEqual([]);
    });

    it('is never held by a right-click, whose menu can swallow the release', () => {
      const { el } = riding();
      el.fire('pointerdown', { button: 2, offsetX: el.clientWidth + 6, offsetY: 200 });
      expect(isScrollbarHeld(el)).toBe(false);
    });

    it('still counts a middle-click in the gutter as a scroll, which pages the track', () => {
      const { el, onScroll } = riding();
      el.fire('pointerdown', { button: 1, offsetX: el.clientWidth + 6, offsetY: 200 });
      expect(isScrollbarHeld(el)).toBe(false);
      platformScrollsTo(el, 900, onScroll);
      expect(followingLiveEdge.value).toBe(false);
    });

    it('announces a release it only learns of from a move with no button held', () => {
      const { el } = riding();
      gutterPress(el);
      el.fire('pointermove', { buttons: 0 });
      expect(isScrollbarHeld(el)).toBe(false);
      expect(released).toEqual([el]);
    });

    it('lets go when the page loses focus, so a lost drag cannot stall the window', () => {
      const { el } = riding();
      gutterPress(el);
      (globalThis as any).document = { hasFocus: () => false };
      fireWindow('blur');
      vi.advanceTimersByTime(0);
      expect(isScrollbarHeld(el)).toBe(false);
      expect(released).toEqual([el]);
    });

    it('keeps holding when focus only moves into an iframe on the page', () => {
      const { el } = riding();
      gutterPress(el);
      (globalThis as any).document = { hasFocus: () => true };
      fireWindow('blur');
      vi.advanceTimersByTime(0);
      expect(isScrollbarHeld(el)).toBe(true);
    });

    it('lets go on teardown, so nothing waiting on the release waits for good', () => {
      const { el, detachGestures } = riding();
      gutterPress(el);
      detachGestures();
      expect(isScrollbarHeld(el)).toBe(false);
      expect(released).toEqual([el]);
    });

    it('stops announcing once unsubscribed', () => {
      const { el } = riding();
      unsubscribe();
      gutterPress(el);
      fireWindow('pointerup');
      expect(released).toEqual([]);
    });

    describe('scrollbarReleased', () => {
      let frames: Array<() => void>;
      let realRaf: any;
      beforeEach(() => {
        frames = [];
        realRaf = (globalThis as any).requestAnimationFrame;
        (globalThis as any).requestAnimationFrame = (fn: () => void) => { frames.push(fn); return frames.length; };
      });
      afterEach(() => { (globalThis as any).requestAnimationFrame = realRaf; });

      const settled = async (p: Promise<void>) => {
        let done = false;
        void p.then(() => { done = true; });
        await Promise.resolve();
        return done;
      };

      it('resolves at once when nothing is held', async () => {
        const { el } = riding();
        expect(await settled(scrollbarReleased(el))).toBe(true);
      });

      it('resolves a frame after the release, so the drag has ended first', async () => {
        const { el } = riding();
        gutterPress(el);
        const landing = scrollbarReleased(el);
        expect(await settled(landing)).toBe(false);

        fireWindow('pointerup');
        expect(await settled(landing)).toBe(false);

        for (const frame of frames.splice(0)) frame();
        expect(await settled(landing)).toBe(true);
      });
    });
  });
});
