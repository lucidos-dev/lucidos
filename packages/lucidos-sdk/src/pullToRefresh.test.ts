import { describe, it, expect, vi, afterEach } from 'vitest';
import {
  APP_PULL_MESSAGE_TYPE,
  APP_REFRESH_MESSAGE_TYPE,
  PULL_REFRESH_THRESHOLD_PX,
  classifyDrag,
  claimsVerticalPan,
  installAppPullToRefresh,
  pullTravel,
  releaseRefreshes,
  scrolledAbove,
  trackPullToRefresh,
  type ScrollNode,
} from './pullToRefresh';

describe('pullTravel', () => {
  it('does not move for an upward or zero drag', () => {
    expect(pullTravel(-40)).toBe(0);
    expect(pullTravel(0)).toBe(0);
  });

  it('moves half as far as the finger', () => {
    expect(pullTravel(60)).toBe(30);
  });

  it('keeps growing past the threshold, so the arrow keeps turning with the finger', () => {
    expect(pullTravel(1_000)).toBe(500);
    expect(pullTravel(1_000)).toBeGreaterThan(PULL_REFRESH_THRESHOLD_PX);
  });
});

describe('releaseRefreshes', () => {
  it('refreshes at the threshold and past it, not before', () => {
    expect(releaseRefreshes(PULL_REFRESH_THRESHOLD_PX - 1)).toBe(false);
    expect(releaseRefreshes(PULL_REFRESH_THRESHOLD_PX)).toBe(true);
  });
});

describe('classifyDrag', () => {
  it('waits until the drag has moved far enough to tell', () => {
    expect(classifyDrag(3, 4)).toBe('undecided');
  });

  it('calls a mostly vertical downward drag a pull', () => {
    expect(classifyDrag(4, 20)).toBe('pull');
  });

  it('never calls a sideways drag a pull, so pane swipes stay intact', () => {
    expect(classifyDrag(30, 12)).toBe('other');
    expect(classifyDrag(-30, 12)).toBe('other');
  });

  it('never calls an upward drag a pull', () => {
    expect(classifyDrag(0, -20)).toBe('other');
  });
});

function chain(...scrollTops: number[]): ScrollNode {
  let parent: ScrollNode | null = null;
  for (const scrollTop of [...scrollTops].reverse()) parent = { scrollTop, parentElement: parent };
  return parent!;
}

describe('claimsVerticalPan', () => {
  it('leaves the vertical pan to the browser by default', () => {
    expect(claimsVerticalPan('auto')).toBe(false);
    expect(claimsVerticalPan('manipulation')).toBe(false);
    expect(claimsVerticalPan('pan-y')).toBe(false);
    expect(claimsVerticalPan('pan-x pan-y')).toBe(false);
  });

  it('is claimed by a surface that handles the drag itself', () => {
    expect(claimsVerticalPan('none')).toBe(true);
    expect(claimsVerticalPan('pan-x')).toBe(true);
    expect(claimsVerticalPan('pinch-zoom')).toBe(true);
  });
});

describe('scrolledAbove', () => {
  it('is false when every ancestor is at the top', () => {
    expect(scrolledAbove(chain(0, 0, 0))).toBe(false);
  });

  it('is true when the scroller is scrolled down', () => {
    expect(scrolledAbove(chain(0, 0, 120))).toBe(true);
  });

  it('is true when an inner list between the target and the scroller is scrolled', () => {
    expect(scrolledAbove(chain(0, 40, 0))).toBe(true);
  });

  it('ignores the negative scrollTop of an iOS top bounce', () => {
    expect(scrolledAbove(chain(0, -12))).toBe(false);
  });
});

// The test env has no TouchEvent, so a plain Event carries the fields read.
function touch(type: string, points: Array<[number, number]>, opts: { target?: ScrollNode; prevented?: boolean } = {}): Event {
  const e = new Event(type);
  Object.defineProperty(e, 'touches', { value: points.map(([clientX, clientY]) => ({ clientX, clientY })) });
  if (opts.target) Object.defineProperty(e, 'target', { value: opts.target });
  if (opts.prevented) Object.defineProperty(e, 'defaultPrevented', { value: true });
  return e;
}

describe('trackPullToRefresh', () => {
  function setup() {
    const surface = new EventTarget();
    const onPull = vi.fn();
    const onRefresh = vi.fn();
    const cleanup = trackPullToRefresh(surface, { onPull, onRefresh });
    const top = chain(0, 0);
    const drag = (points: Array<[number, number]>, opts: { target?: ScrollNode; prevented?: boolean } = {}) => {
      const [first, ...rest] = points;
      surface.dispatchEvent(touch('touchstart', [first], { target: opts.target ?? top }));
      for (const p of rest) surface.dispatchEvent(touch('touchmove', [p], { prevented: opts.prevented }));
    };
    const release = () => surface.dispatchEvent(touch('touchend', []));
    return { surface, onPull, onRefresh, cleanup, drag, release };
  }

  it('refreshes on a release past the threshold, and retracts first', () => {
    const t = setup();
    t.drag([[100, 0], [100, 20], [100, 200]]);
    expect(t.onPull).toHaveBeenLastCalledWith(100);
    t.release();
    expect(t.onPull).toHaveBeenLastCalledWith(0);
    expect(t.onRefresh).toHaveBeenCalledTimes(1);
  });

  it('springs back without refreshing on a short pull', () => {
    const t = setup();
    t.drag([[100, 0], [100, 20], [100, 60]]);
    t.release();
    expect(t.onPull).toHaveBeenLastCalledWith(0);
    expect(t.onRefresh).not.toHaveBeenCalled();
  });

  it('never arms while the scroller stays below the top', () => {
    const t = setup();
    t.drag([[100, 0], [100, 20], [100, 200]], { target: chain(0, 300) });
    t.release();
    expect(t.onPull).not.toHaveBeenCalled();
    expect(t.onRefresh).not.toHaveBeenCalled();
  });

  it('arms when a scroll up reaches the top, measuring from that point', () => {
    // The unreliable pull: a scroll up that runs into the top and carries on.
    const t = setup();
    const target = chain(0, 300);
    const scroller = target.parentElement!;
    t.surface.dispatchEvent(touch('touchstart', [[100, 0]], { target }));
    t.surface.dispatchEvent(touch('touchmove', [[100, 150]]));
    scroller.scrollTop = 0;
    t.surface.dispatchEvent(touch('touchmove', [[100, 300]]));
    expect(t.onPull).not.toHaveBeenCalled();
    t.surface.dispatchEvent(touch('touchmove', [[100, 320]]));
    t.surface.dispatchEvent(touch('touchmove', [[100, 300 + 2 * PULL_REFRESH_THRESHOLD_PX]]));
    expect(t.onPull).toHaveBeenLastCalledWith(PULL_REFRESH_THRESHOLD_PX);
    t.release();
    expect(t.onRefresh).toHaveBeenCalledTimes(1);
  });

  it('does not arm when the scroll reaches the top and the finger turns back up', () => {
    const t = setup();
    const target = chain(0, 300);
    t.surface.dispatchEvent(touch('touchstart', [[100, 0]], { target }));
    target.parentElement!.scrollTop = 0;
    t.surface.dispatchEvent(touch('touchmove', [[100, 150]]));
    t.surface.dispatchEvent(touch('touchmove', [[100, 60]]));
    t.release();
    expect(t.onPull).not.toHaveBeenCalled();
    expect(t.onRefresh).not.toHaveBeenCalled();
  });

  it('ignores a sideways swipe', () => {
    const t = setup();
    t.drag([[100, 0], [160, 10], [260, 200]]);
    t.release();
    expect(t.onPull).not.toHaveBeenCalled();
    expect(t.onRefresh).not.toHaveBeenCalled();
  });

  it('abandons a pull that another handler claimed', () => {
    const t = setup();
    t.drag([[100, 0], [100, 20]]);
    t.surface.dispatchEvent(touch('touchmove', [[100, 200]], { prevented: true }));
    t.release();
    expect(t.onPull).toHaveBeenLastCalledWith(0);
    expect(t.onRefresh).not.toHaveBeenCalled();
  });

  it('retracts a pull whose touchend never arrived when the next touch starts', () => {
    const t = setup();
    t.drag([[100, 0], [100, 20], [100, 120]]);
    expect(t.onPull).toHaveBeenLastCalledWith(60);
    t.surface.dispatchEvent(touch('touchstart', [[50, 50]], { target: chain(0, 0) }));
    expect(t.onPull).toHaveBeenLastCalledWith(0);
    expect(t.onRefresh).not.toHaveBeenCalled();
  });

  it('abandons a pull when a second finger lands', () => {
    const t = setup();
    t.drag([[100, 0], [100, 200]]);
    t.surface.dispatchEvent(touch('touchmove', [[100, 200], [200, 200]]));
    t.release();
    expect(t.onRefresh).not.toHaveBeenCalled();
  });

  it('listens passively, so it can never block a native scroll', () => {
    const surface = new EventTarget();
    const add = vi.spyOn(surface, 'addEventListener');
    trackPullToRefresh(surface, { onPull: () => {}, onRefresh: () => {} });
    expect(add.mock.calls.length).toBeGreaterThan(0);
    for (const call of add.mock.calls) expect(call[2]).toMatchObject({ passive: true });
  });

  it('stops listening after cleanup', () => {
    const t = setup();
    t.cleanup();
    t.drag([[100, 0], [100, 20], [100, 200]]);
    t.release();
    expect(t.onPull).not.toHaveBeenCalled();
  });
});

describe('installAppPullToRefresh', () => {
  let cleanup: () => void = () => {};

  afterEach(() => {
    cleanup();
    cleanup = () => {};
    delete (window as unknown as { parent?: unknown }).parent;
  });

  it('posts the travel and the release to the host', () => {
    const postMessage = vi.fn();
    (window as unknown as { parent: unknown }).parent = { postMessage };
    cleanup = installAppPullToRefresh();
    const top = chain(0);
    window.dispatchEvent(touch('touchstart', [[100, 0]], { target: top }));
    window.dispatchEvent(touch('touchmove', [[100, 20]]));
    window.dispatchEvent(touch('touchmove', [[100, 200]]));
    window.dispatchEvent(touch('touchend', []));
    expect(postMessage).toHaveBeenCalledWith({ type: APP_PULL_MESSAGE_TYPE, travel: 100 }, '*');
    expect(postMessage).toHaveBeenLastCalledWith({ type: APP_REFRESH_MESSAGE_TYPE }, '*');
  });

  it('does nothing at the top level, where there is no host', () => {
    (window as unknown as { parent: unknown }).parent = window;
    const add = vi.spyOn(window, 'addEventListener');
    cleanup = installAppPullToRefresh();
    expect(add).not.toHaveBeenCalledWith('touchstart', expect.anything(), expect.anything());
    add.mockRestore();
  });
});
