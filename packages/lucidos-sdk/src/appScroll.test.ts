import { describe, it, expect, vi, afterEach, beforeEach } from 'vitest';
import {
  APP_SCROLL_MESSAGE_TYPE,
  APP_TOUCH_MESSAGE_TYPE,
  clampedTop,
  createScrollTracker,
  installAppScrollReport,
  mergeReports,
  type ScrollBox,
} from './appScroll';

function box(scrollTop: number, scrollHeight = 2000, clientHeight = 600): ScrollBox {
  return { scrollTop, scrollHeight, clientHeight };
}

describe('clampedTop', () => {
  it('clamps elastic bounce past either end', () => {
    expect(clampedTop(box(-30))).toBe(0);
    expect(clampedTop(box(1500))).toBe(1400);
    expect(clampedTop(box(700))).toBe(700);
  });
});

describe('createScrollTracker', () => {
  it("takes an element's first scroll as its baseline, so a restore on load moves nothing", () => {
    const tracker = createScrollTracker();
    expect(tracker.scrolled(box(900), false)).toEqual({ delta: 0, top: 900, max: 1400 });
  });

  it('measures each later scroll against the same element', () => {
    const tracker = createScrollTracker();
    const el = box(100);
    tracker.scrolled(el, false);
    el.scrollTop = 160;
    expect(tracker.scrolled(el, false).delta).toBe(60);
    el.scrollTop = 120;
    expect(tracker.scrolled(el, false).delta).toBe(-40);
  });

  it('keeps one baseline per element', () => {
    const tracker = createScrollTracker();
    const root = box(500);
    const list = box(0, 800, 200);
    tracker.scrolled(root, false);
    expect(tracker.scrolled(list, false).delta).toBe(0);
    list.scrollTop = 50;
    expect(tracker.scrolled(list, false).delta).toBe(50);
  });

  it('re-bases without a delta on the clamp a viewport resize causes', () => {
    const tracker = createScrollTracker();
    const el = box(1400);
    tracker.scrolled(el, false);
    el.clientHeight = 650;
    el.scrollTop = 1350;
    expect(tracker.scrolled(el, true).delta).toBe(0);
    el.scrollTop = 1300;
    expect(tracker.scrolled(el, false).delta).toBe(-50);
  });

  it('knows a clamp by its shape, with no resize in sight', () => {
    const tracker = createScrollTracker();
    const el = box(1400);
    tracker.scrolled(el, false);
    // The frame grew, so its bottom came down to meet the old position.
    el.clientHeight = 650;
    el.scrollTop = 1350;
    expect(tracker.scrolled(el, false).delta).toBe(0);
    el.scrollTop = 1300;
    expect(tracker.scrolled(el, false).delta).toBe(-50);
  });

  it('knows a clamp that stops a fraction short of the bottom', () => {
    const tracker = createScrollTracker();
    const el = box(1400);
    tracker.scrolled(el, false);
    el.clientHeight = 650;
    el.scrollTop = 1349.48;
    expect(tracker.scrolled(el, false).delta).toBe(0);
  });

  it("counts the reader's own scroll back to the bottom", () => {
    const tracker = createScrollTracker();
    const el = box(1400);
    tracker.scrolled(el, false);
    el.scrollTop = 1300;
    expect(tracker.scrolled(el, false).delta).toBe(-100);
    el.scrollTop = 1400;
    expect(tracker.scrolled(el, false).delta).toBe(100);
  });
});

describe('mergeReports', () => {
  it('adds the deltas and keeps the later position', () => {
    expect(mergeReports({ delta: 10, top: 110, max: 900 }, { delta: 5, top: 115, max: 900 }))
      .toEqual({ delta: 15, top: 115, max: 900 });
    expect(mergeReports(null, { delta: 5, top: 115, max: 900 })).toEqual({ delta: 5, top: 115, max: 900 });
  });
});

describe('installAppScrollReport', () => {
  let cleanup: () => void = () => {};
  let frames: FrameRequestCallback[] = [];
  let onScroll: (e: { target: unknown }) => void = () => {};
  let onResize: () => void = () => {};
  let windowListeners: Map<string, { listener: (e: unknown) => void; options: unknown }> = new Map();
  const root = box(0, 3000, 600);
  const runFrame = () => {
    const pending = frames;
    frames = [];
    for (const cb of pending) cb(0);
  };
  const scrollTo = (top: number) => {
    root.scrollTop = top;
    onScroll({ target: root });
  };
  const install = (parent: unknown) => {
    (window as unknown as { parent: unknown }).parent = parent;
    const addDoc = vi.spyOn(document, 'addEventListener');
    const addWin = vi.spyOn(window, 'addEventListener');
    cleanup = installAppScrollReport();
    onScroll = addDoc.mock.calls.find(([type]) => type === 'scroll')?.[1] as typeof onScroll;
    onResize = addWin.mock.calls.find(([type]) => type === 'resize')?.[1] as typeof onResize;
    windowListeners = new Map(addWin.mock.calls.map(([type, listener, options]) =>
      [type, { listener: listener as (e: unknown) => void, options }]));
    return addDoc;
  };

  beforeEach(() => {
    root.scrollTop = 0;
    root.clientHeight = 600;
    vi.spyOn(window, 'requestAnimationFrame').mockImplementation((cb) => {
      frames.push(cb);
      return frames.length;
    });
  });

  afterEach(() => {
    cleanup();
    cleanup = () => {};
    frames = [];
    vi.restoreAllMocks();
    delete (window as unknown as { parent?: unknown }).parent;
  });

  it('posts one report per frame, with the deltas added up', () => {
    const postMessage = vi.fn();
    install({ postMessage });
    scrollTo(100);
    runFrame();
    expect(postMessage).not.toHaveBeenCalled();
    scrollTo(130);
    scrollTo(170);
    runFrame();
    expect(postMessage).toHaveBeenCalledTimes(1);
    expect(postMessage).toHaveBeenCalledWith({ type: APP_SCROLL_MESSAGE_TYPE, delta: 70, top: 170, max: 2400 }, '*');
  });

  it('posts nothing for a scroll in the frame of a resize', () => {
    const postMessage = vi.fn();
    install({ postMessage });
    scrollTo(500);
    runFrame();
    onResize();
    scrollTo(440);
    runFrame();
    expect(postMessage).not.toHaveBeenCalled();
    scrollTo(400);
    runFrame();
    expect(postMessage).toHaveBeenCalledWith(expect.objectContaining({ delta: -40 }), '*');
  });

  it('posts nothing for a clamp that lands after the frame of the resize', () => {
    const postMessage = vi.fn();
    install({ postMessage });
    scrollTo(500);
    runFrame();
    scrollTo(2400);
    runFrame();
    postMessage.mockClear();
    onResize();
    runFrame();
    root.clientHeight = 650;
    scrollTo(2350);
    runFrame();
    expect(postMessage).not.toHaveBeenCalled();
  });

  it("keeps the vertical scroller's position when a sideways scroll lands in the same frame", () => {
    const postMessage = vi.fn();
    install({ postMessage });
    const carousel = box(0, 200, 200);
    scrollTo(100);
    runFrame();
    onScroll({ target: carousel });
    runFrame();
    scrollTo(160);
    onScroll({ target: carousel });
    runFrame();
    expect(postMessage).toHaveBeenCalledWith({ type: APP_SCROLL_MESSAGE_TYPE, delta: 60, top: 160, max: 2400 }, '*');
  });

  it('only listens, passively, so the app keeps its native scroll', () => {
    const add = install({ postMessage: vi.fn() });
    expect(add).toHaveBeenCalledWith('scroll', expect.any(Function), { capture: true, passive: true });
  });

  describe('the finger', () => {
    const touches = (length: number) => ({ touches: { length }, preventDefault: vi.fn() });
    const fire = (type: string, length = 0) => {
      const event = touches(length);
      windowListeners.get(type)!.listener(event);
      return event;
    };
    const posted = (postMessage: ReturnType<typeof vi.fn>) =>
      postMessage.mock.calls.filter(([m]) => m.type === APP_TOUCH_MESSAGE_TYPE).map(([m]) => m.touching);

    it('reports a finger landing on the app and lifting off it', () => {
      const postMessage = vi.fn();
      install({ postMessage });
      fire('touchstart', 1);
      expect(postMessage).toHaveBeenLastCalledWith({ type: APP_TOUCH_MESSAGE_TYPE, touching: true }, '*');
      fire('touchend', 0);
      expect(posted(postMessage)).toEqual([true, false]);
    });

    it('stays touching until the last finger lifts', () => {
      const postMessage = vi.fn();
      install({ postMessage });
      fire('touchstart', 1);
      fire('touchstart', 2);
      fire('touchend', 1);
      expect(posted(postMessage)).toEqual([true]);
      fire('touchcancel', 0);
      expect(posted(postMessage)).toEqual([true, false]);
    });

    it('lifts the finger when the page goes away mid-touch', () => {
      const postMessage = vi.fn();
      install({ postMessage });
      fire('touchstart', 1);
      windowListeners.get('pagehide')!.listener({});
      expect(posted(postMessage)).toEqual([true, false]);
    });

    it('only listens, passively, in the capture phase, and never cancels a touch', () => {
      install({ postMessage: vi.fn() });
      for (const type of ['touchstart', 'touchend', 'touchcancel']) {
        expect(windowListeners.get(type)?.options).toEqual({ capture: true, passive: true });
      }
      expect(fire('touchstart', 1).preventDefault).not.toHaveBeenCalled();
    });
  });

  it('does nothing at the top level, where no host listens', () => {
    const add = install(window);
    expect(add).not.toHaveBeenCalledWith('scroll', expect.anything(), expect.anything());
  });
});
