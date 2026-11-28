import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

if (typeof (globalThis as any).HTMLElement === 'undefined') {
  (globalThis as any).HTMLElement = class {};
}

import {
  followingLiveEdge,
  holdAcrossRelayout,
  isScrollHeld,
  makeScrollObservers,
  onScrollHoldReleased,
  readerGestureForTest,
  scrollHoldReleased,
  setActiveScrollElement,
  setFollowLiveEdge,
  setTranscriptLive,
  stopFollowingBottom,
  TOUCH_COAST_IDLE_MS,
  TOUCH_COAST_MAX_MS,
} from '../scrollState';
import { READER_SCROLL_EVENT_WAIT_MS } from '../readerInput';

describe('what the reader-gesture listeners count as a scroll', () => {
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

  function riding() {
    const el = makeContainer();
    const observers = makeScrollObservers(el);
    setActiveScrollElement(el);
    setFollowLiveEdge(true);
    vi.advanceTimersByTime(1500);
    expect(followingLiveEdge.value).toBe(true);
    return { el, ...observers };
  }

  function platformScrollsTo(el: any, top: number, onScroll: () => void) {
    el.scrollTop = top;
    onScroll();
  }

  function gutterPress(el: any) {
    el.fire('pointerdown', { offsetX: el.clientWidth + 6, offsetY: 200 });
  }

  function fireWindow(type: string, event: Record<string, unknown> = {}) {
    for (const fn of windowListeners[type] ?? []) fn(event);
  }

  function ridingAndAnchored() {
    const r = riding();
    r.onScroll();
    return r;
  }

  it('attaches to the container, and the releases to window', () => {
    const el = makeContainer();
    const { detachGestures } = makeScrollObservers(el);

    expect(Object.keys(el.listeners).sort()).toEqual(
      ['focusin', 'keydown', 'pointerdown', 'pointermove', 'scroll', 'touchmove', 'touchstart', 'wheel'],
    );
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

  it('never writes the edge between a wheel notch and its scroll event', () => {
    const { el, onScroll, onResize } = riding();
    const before = el.scrollTop;
    el.fire('wheel');
    el.scrollHeight += 60;
    onResize();
    expect(el.scrollTop).toBe(before);
    platformScrollsTo(el, before - 200, onScroll);
    expect(followingLiveEdge.value).toBe(false);
  });

  it('keeps the edge once a wheel notch that moved nothing has waited', () => {
    const { el, onResize } = riding();
    el.fire('wheel');
    el.scrollHeight += 60;
    onResize();
    vi.advanceTimersByTime(READER_SCROLL_EVENT_WAIT_MS + 1);
    expect(el.scrollTop).toBe(el.scrollHeight - el.clientHeight);
    expect(followingLiveEdge.value).toBe(true);
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
    const { el, onScroll } = ridingAndAnchored();

    el.fire('keydown', { key: 'PageUp', target: { nodeName: 'BUTTON' } });
    platformScrollsTo(el, 900, onScroll);

    expect(el.scrollTop).toBe(900);
    expect(followingLiveEdge.value).toBe(false);
  });

  it('and answers the platform again once that keypress is four frames old', () => {
    const { el, onScroll } = ridingAndAnchored();

    el.fire('keydown', { key: 'PageUp', target: { nodeName: 'BUTTON' } });
    vi.advanceTimersByTime(200);
    platformScrollsTo(el, 900, onScroll);

    expect(el.scrollTop).toBe(2500);
    expect(followingLiveEdge.value).toBe(true);
  });

  it('keeps the reader where FOCUS landing inside it took them, and ends the ride', () => {
    const { el, onScroll, onResize } = ridingAndAnchored();
    const buttonInAnOlderTurn = { nodeName: 'BUTTON' };

    el.fire('focusin', { target: buttonInAnOlderTurn });
    platformScrollsTo(el, 400, onScroll);

    expect(el.scrollTop).toBe(400);
    expect(followingLiveEdge.value).toBe(false);

    el.scrollHeight = 3100;
    onResize();
    expect(el.scrollTop).toBe(400);
  });

  it('and answers the platform again once the focus reveal is four frames old', () => {
    const { el, onScroll } = ridingAndAnchored();

    el.fire('focusin', { target: { nodeName: 'BUTTON' } });
    vi.advanceTimersByTime(200);
    platformScrollsTo(el, 400, onScroll);

    expect(el.scrollTop).toBe(2500);
    expect(followingLiveEdge.value).toBe(true);
  });

  it('does NOT read focus on a control already in full view as a placement', () => {
    const { el, onScroll } = ridingAndAnchored();
    const tappedOption = {
      nodeName: 'BUTTON',
      getBoundingClientRect: () => ({ width: 300, height: 40, top: 400, bottom: 440, left: 20, right: 320 }),
    };

    el.fire('focusin', { target: tappedOption });
    platformScrollsTo(el, 400, onScroll);

    expect(el.scrollTop).toBe(2500);
    expect(followingLiveEdge.value).toBe(true);
  });

  it('does NOT read focus on the transcript itself as a placement', () => {
    const { el, onScroll } = ridingAndAnchored();

    el.fire('focusin', { target: el });
    platformScrollsTo(el, 400, onScroll);

    expect(el.scrollTop).toBe(2500);
    expect(followingLiveEdge.value).toBe(true);
  });

  it('counts a scrollbar DRAG for as long as the thumb is held', () => {
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
    const { el, onScroll } = riding();
    el.fire('pointermove', { buttons: 0 });
    platformScrollsTo(el, 900, onScroll);
    expect(followingLiveEdge.value).toBe(true);
  });

  it('does NOT count the jitter every real click on a control carries', () => {
    const { el, onScroll } = riding();
    const answerButton = { nodeName: 'BUTTON' };

    el.fire('pointerdown', { target: answerButton, offsetX: 40, offsetY: 12 });
    el.fire('pointermove', { buttons: 1 });
    el.fire('pointermove', { buttons: 1 });
    platformScrollsTo(el, 900, onScroll);

    expect(followingLiveEdge.value).toBe(true);
  });

  it('counts a press in the scrollbar GUTTER, which scrolls with no movement', () => {
    const { el, onScroll } = riding();
    gutterPress(el);
    platformScrollsTo(el, 900, onScroll);
    expect(followingLiveEdge.value).toBe(false);
  });

  it('does NOT count an ordinary press, wherever in the transcript it lands', () => {
    const { el, onScroll } = riding();
    const tallTurn = { nodeName: 'DIV' };

    el.fire('pointerdown', { target: tallTurn, offsetX: 900, offsetY: 4000 });
    platformScrollsTo(el, 900, onScroll);

    expect(followingLiveEdge.value).toBe(true);
  });

  it('forgets a scrollbar press released where it could not see it', () => {
    const { el, onScroll } = riding();

    gutterPress(el);
    el.fire('pointermove', { buttons: 0 });
    vi.advanceTimersByTime(2000);
    el.fire('pointermove', { buttons: 0 });
    platformScrollsTo(el, 900, onScroll);

    expect(followingLiveEdge.value).toBe(true);
  });

  it('ends the press on a release anywhere, including outside the transcript', () => {
    const { el, onScroll } = riding();

    gutterPress(el);
    fireWindow('pointerup');
    vi.advanceTimersByTime(2000);
    el.fire('pointermove', { buttons: 1 });
    platformScrollsTo(el, 900, onScroll);

    expect(followingLiveEdge.value).toBe(true);
  });

  describe('a held scrollbar, which the window waits out (ADR 0258)', () => {
    let released: unknown[];
    let unsubscribe: () => void;
    let realDocument: any;
    beforeEach(() => {
      released = [];
      unsubscribe = onScrollHoldReleased((el) => released.push(el));
      realDocument = (globalThis as any).document;
    });
    afterEach(() => {
      unsubscribe();
      (globalThis as any).document = realDocument;
    });

    it('is held from a gutter press until the release, which is announced once', () => {
      const { el } = riding();
      gutterPress(el);
      expect(isScrollHeld(el)).toBe(true);

      fireWindow('pointerup');
      expect(isScrollHeld(el)).toBe(false);
      expect(released).toEqual([el]);

      fireWindow('pointerup');
      expect(released).toEqual([el]);
    });

    it('is held by a press on the container inside its client box, where an overlay thumb sits', () => {
      const { el } = ridingAndAnchored();
      el.fire('pointerdown', { offsetX: el.clientWidth - 4, offsetY: 200 });
      expect(isScrollHeld(el)).toBe(true);
    });

    it('leaves the reader where an overlay thumb drag put them', () => {
      const { el, onScroll } = ridingAndAnchored();
      el.fire('pointerdown', { offsetX: el.clientWidth - 4, offsetY: 200 });
      platformScrollsTo(el, 400, onScroll);
      expect(el.scrollTop).toBe(400);
      expect(followingLiveEdge.value).toBe(false);
    });

    it('retires the ride on an overlay thumb drag, however slow', () => {
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
      expect(isScrollHeld(el)).toBe(false);
      fireWindow('pointerup');
      expect(released).toEqual([]);
    });

    it('is never held by a right-click, whose menu can swallow the release', () => {
      const { el } = riding();
      el.fire('pointerdown', { button: 2, offsetX: el.clientWidth + 6, offsetY: 200 });
      expect(isScrollHeld(el)).toBe(false);
    });

    it('still counts a middle-click in the gutter as a scroll, which pages the track', () => {
      const { el, onScroll } = riding();
      el.fire('pointerdown', { button: 1, offsetX: el.clientWidth + 6, offsetY: 200 });
      expect(isScrollHeld(el)).toBe(false);
      platformScrollsTo(el, 900, onScroll);
      expect(followingLiveEdge.value).toBe(false);
    });

    it('announces a release it only learns of from a move with no button held', () => {
      const { el } = riding();
      gutterPress(el);
      el.fire('pointermove', { buttons: 0 });
      expect(isScrollHeld(el)).toBe(false);
      expect(released).toEqual([el]);
    });

    it('lets go when the page loses focus, so a lost drag cannot stall the window', () => {
      const { el } = riding();
      gutterPress(el);
      (globalThis as any).document = { hasFocus: () => false };
      fireWindow('blur');
      vi.advanceTimersByTime(0);
      expect(isScrollHeld(el)).toBe(false);
      expect(released).toEqual([el]);
    });

    it('keeps holding when focus only moves into an iframe on the page', () => {
      const { el } = riding();
      gutterPress(el);
      (globalThis as any).document = { hasFocus: () => true };
      fireWindow('blur');
      vi.advanceTimersByTime(0);
      expect(isScrollHeld(el)).toBe(true);
    });

    it('lets go on teardown, so nothing waiting on the release waits for good', () => {
      const { el, detachGestures } = riding();
      gutterPress(el);
      detachGestures();
      expect(isScrollHeld(el)).toBe(false);
      expect(released).toEqual([el]);
    });

    it('stops announcing once unsubscribed', () => {
      const { el } = riding();
      unsubscribe();
      gutterPress(el);
      fireWindow('pointerup');
      expect(released).toEqual([]);
    });

    describe('scrollHoldReleased', () => {
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
        expect(await settled(scrollHoldReleased(el))).toBe(true);
      });

      it('resolves a frame after the release, so the drag has ended first', async () => {
        const { el } = riding();
        gutterPress(el);
        const landing = scrollHoldReleased(el);
        expect(await settled(landing)).toBe(false);

        fireWindow('pointerup');
        expect(await settled(landing)).toBe(false);

        for (const frame of frames.splice(0)) frame();
        expect(await settled(landing)).toBe(true);
      });
    });
  });

  describe('a touch scroll, held until its momentum stops', () => {
    let released: unknown[];
    let unsubscribe: () => void;
    beforeEach(() => {
      released = [];
      unsubscribe = onScrollHoldReleased((el) => released.push(el));
    });
    afterEach(() => unsubscribe());

    const lift = (remaining = 0) => fireWindow('touchend', { touches: new Array(remaining) });

    it('is never held by a tap, which moves nothing', () => {
      const { el } = riding();
      el.fire('touchstart');
      lift();
      expect(isScrollHeld(el)).toBe(false);
      expect(released).toEqual([]);
    });

    it('holds from the first move while the finger stays down', () => {
      const { el } = riding();
      el.fire('touchmove');
      expect(isScrollHeld(el)).toBe(true);
      vi.advanceTimersByTime(TOUCH_COAST_IDLE_MS * 10);
      expect(isScrollHeld(el)).toBe(true);
    });

    it('keeps holding while the momentum scrolls, and lets go once it is still', () => {
      const { el } = riding();
      el.fire('touchmove');
      lift();
      for (let i = 0; i < 20; i++) {
        vi.advanceTimersByTime(16);
        el.fire('scroll');
      }
      expect(isScrollHeld(el)).toBe(true);
      vi.advanceTimersByTime(TOUCH_COAST_IDLE_MS);
      expect(isScrollHeld(el)).toBe(false);
      expect(released).toEqual([el]);
    });

    it('waits for the last finger to lift', () => {
      const { el } = riding();
      el.fire('touchmove');
      lift(1);
      vi.advanceTimersByTime(TOUCH_COAST_IDLE_MS * 2);
      expect(isScrollHeld(el)).toBe(true);
      lift();
      vi.advanceTimersByTime(TOUCH_COAST_IDLE_MS);
      expect(isScrollHeld(el)).toBe(false);
    });

    it('holds a fling beyond four seconds until scroll events go quiet', () => {
      const { el } = riding();
      el.fire('touchmove');
      lift();
      for (let t = 0; t <= TOUCH_COAST_MAX_MS; t += 50) {
        el.fire('scroll');
        vi.advanceTimersByTime(50);
      }
      expect(isScrollHeld(el)).toBe(true);
      expect(released).toEqual([]);
      vi.advanceTimersByTime(TOUCH_COAST_IDLE_MS);
      expect(isScrollHeld(el)).toBe(false);
      expect(released).toEqual([el]);
    });

    it('holds a shifted reading line only after a long fling goes quiet', () => {
      const el = makeContainer();
      let blockTop = 100;
      el.children = [{
        getBoundingClientRect: () => ({ top: blockTop - el.scrollTop, height: 1500 }),
      }];
      el.scrollTop = 500;
      const { onResize, onScroll, detachGestures } = makeScrollObservers(el);
      onScroll();

      el.fire('touchmove');
      blockTop += 100;
      onResize();
      expect(el.scrollTop).toBe(500);

      lift();
      for (let elapsed = 0; elapsed <= TOUCH_COAST_MAX_MS; elapsed += 50) {
        el.scrollTop += 1;
        el.fire('scroll');
        onScroll();
        vi.advanceTimersByTime(50);
      }

      const flungTo = el.scrollTop;
      blockTop += 100;
      onResize();
      expect(el.scrollTop).toBe(flungTo);

      vi.advanceTimersByTime(TOUCH_COAST_IDLE_MS);
      onResize();
      expect(el.scrollTop).toBe(flungTo + 100);
      detachGestures();
    });

    it('carries a history correction to the spot where the fling ends', () => {
      const el = makeContainer();
      el.scrollTop = 500;
      const { onScroll, detachGestures } = makeScrollObservers(el);
      onScroll();

      el.fire('touchmove');
      el.scrollHeight += 250;
      holdAcrossRelayout(el, el.scrollTop + 250);
      expect(el.scrollTop).toBe(500);

      lift();
      el.scrollTop = 700;
      el.fire('scroll');
      onScroll();
      vi.advanceTimersByTime(TOUCH_COAST_IDLE_MS);

      expect(el.scrollTop).toBe(950);
      detachGestures();
    });

    it('lets go at the cap when the lift never reaches the window', () => {
      const { el } = riding();
      el.fire('touchmove');
      vi.advanceTimersByTime(TOUCH_COAST_MAX_MS);
      expect(isScrollHeld(el)).toBe(false);
      expect(released).toEqual([el]);
    });

    it('keeps holding a long drag, since every move re-arms the cap', () => {
      const { el } = riding();
      for (let t = 0; t < TOUCH_COAST_MAX_MS * 2; t += 50) {
        el.fire('touchmove');
        vi.advanceTimersByTime(50);
      }
      expect(isScrollHeld(el)).toBe(true);
    });

    it('drops the first cap when a tap lands mid-coast, so it cannot end a later drag', () => {
      const { el } = riding();
      el.fire('touchmove');
      lift();
      vi.advanceTimersByTime(TOUCH_COAST_IDLE_MS / 2);
      lift();
      vi.advanceTimersByTime(TOUCH_COAST_IDLE_MS);
      expect(released).toEqual([el]);
      vi.advanceTimersByTime(TOUCH_COAST_MAX_MS - TOUCH_COAST_IDLE_MS * 3);
      el.fire('touchmove');
      vi.advanceTimersByTime(TOUCH_COAST_IDLE_MS * 4);
      expect(isScrollHeld(el)).toBe(true);
      expect(released).toEqual([el]);
    });

    it('holds the landing gate until the coast is over', async () => {
      const frames: Array<() => void> = [];
      const realRaf = (globalThis as any).requestAnimationFrame;
      (globalThis as any).requestAnimationFrame = (fn: () => void) => { frames.push(fn); return frames.length; };
      try {
        const { el } = riding();
        el.fire('touchmove');
        let landed = false;
        void scrollHoldReleased(el).then(() => { landed = true; });
        lift();
        el.fire('scroll');
        await Promise.resolve();
        expect(landed).toBe(false);
        vi.advanceTimersByTime(TOUCH_COAST_IDLE_MS);
        for (const frame of frames.splice(0)) frame();
        await Promise.resolve();
        expect(landed).toBe(true);
      } finally {
        (globalThis as any).requestAnimationFrame = realRaf;
      }
    });

    it('announces once when a scrollbar and a touch hold end together on teardown', () => {
      const { el, detachGestures } = riding();
      el.fire('pointerdown', { offsetX: el.clientWidth + 6, offsetY: 200 });
      el.fire('touchmove');
      detachGestures();
      expect(isScrollHeld(el)).toBe(false);
      expect(released).toEqual([el]);
    });
  });
});
