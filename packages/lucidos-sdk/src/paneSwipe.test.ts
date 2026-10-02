import { describe, it, expect, vi, afterEach } from 'vitest';
import {
  APP_SWIPE_END_MESSAGE_TYPE,
  APP_SWIPE_MESSAGE_TYPE,
  SwipeTouch,
  claimsHorizontalPan,
  horizontalDragClaimed,
  installAppPaneSwipe,
  trackPaneSwipe,
  type DragNode,
  type PaneSwipeEnv,
  type StyleOf,
} from './paneSwipe';

describe('SwipeTouch', () => {
  it('starts dx at 0 when crossing the lock threshold (no dead-zone jump)', () => {
    // Without subtracting LOCK_THRESHOLD on lock, the first frame after the
    // 8px direction-lock jumps the image by 8px — visible as a "snap".
    const s = new SwipeTouch();
    s.start(0, 0);
    expect(s.move(7, 0)).toBeNull();          // below threshold, undecided
    expect(s.move(8, 0)).toBe(0);              // locks at threshold → start from 0
    expect(s.move(20, 0)).toBe(12);            // 20 - 8 = 12px of visible drag
  });

  it('returns null while moving vertically', () => {
    const s = new SwipeTouch();
    s.start(0, 0);
    expect(s.move(0, 20)).toBeNull();
    expect(s.move(5, 30)).toBeNull();
  });

  it('commits a fast horizontal swipe to the right as previous', () => {
    const s = new SwipeTouch();
    s.start(0, 0);
    s.move(50, 0);
    expect(s.end(800)).toBe(-1);               // finger right → previous
  });
});

describe('claimsHorizontalPan', () => {
  it('leaves the default pan to the pane swipe', () => {
    expect(claimsHorizontalPan('')).toBe(false);
    expect(claimsHorizontalPan('auto')).toBe(false);
    expect(claimsHorizontalPan('manipulation')).toBe(false);
  });

  it('reads a declaration that keeps the sideways pan as a claim', () => {
    expect(claimsHorizontalPan('none')).toBe(true);
    expect(claimsHorizontalPan('pan-y')).toBe(true);
    expect(claimsHorizontalPan('pan-y pinch-zoom')).toBe(true);
  });

  it('leaves a native sideways pan alone, since only a real scroller claims it', () => {
    expect(claimsHorizontalPan('pan-x')).toBe(false);
  });
});

type Node = DragNode & { overflowX?: string; touchAction?: string };

/** A leaf inside a chain of ancestors, innermost first. */
function chain(...nodes: Array<Omit<Node, 'parentElement'>>): Node {
  let parent: Node | null = null;
  for (const n of [...nodes].reverse()) parent = { ...n, parentElement: parent };
  return parent!;
}

const styleOf: StyleOf = (node) => {
  const n = node as Node;
  return { overflowX: n.overflowX ?? 'visible', touchAction: n.touchAction ?? 'auto' };
};

describe('horizontalDragClaimed', () => {
  it('is free on ordinary content', () => {
    expect(horizontalDragClaimed(chain({ localName: 'p' }, { localName: 'body' }), styleOf)).toBe(false);
  });

  it('is claimed by a range input', () => {
    expect(horizontalDragClaimed(chain({ localName: 'input', type: 'range' }), styleOf)).toBe(true);
  });

  it('is claimed by an ancestor that scrolls sideways', () => {
    const strip = { localName: 'div', overflowX: 'auto', scrollWidth: 900, clientWidth: 300 };
    expect(horizontalDragClaimed(chain({ localName: 'img' }, strip), styleOf)).toBe(true);
  });

  it('is free on a sideways scroller with nothing to scroll', () => {
    const strip = { localName: 'div', overflowX: 'auto', scrollWidth: 300, clientWidth: 300 };
    expect(horizontalDragClaimed(chain({ localName: 'img' }, strip), styleOf)).toBe(false);
  });

  it('is claimed by a touch-action on an ancestor', () => {
    expect(horizontalDragClaimed(chain({ localName: 'div' }, { localName: 'canvas', touchAction: 'none' }), styleOf)).toBe(true);
  });
});

// The test env has no TouchEvent, so a plain Event carries the fields read.
// Points are screen coordinates. `frameShift` is how far the host has moved
// the frame, which moves the client coordinates the frame sees and nothing else.
function touch(
  type: string,
  points: Array<[number, number]>,
  opts: { target?: DragNode; prevented?: boolean; cancelable?: boolean; frameShift?: number } = {},
): Event {
  const e = new Event(type, { cancelable: opts.cancelable ?? true });
  const shift = opts.frameShift ?? 0;
  const touches = points.map(([screenX, screenY]) => ({ screenX, screenY, clientX: screenX - shift, clientY: screenY }));
  Object.defineProperty(e, 'touches', { value: touches });
  if (opts.target) Object.defineProperty(e, 'target', { value: opts.target });
  if (opts.prevented) Object.defineProperty(e, 'defaultPrevented', { value: true });
  return e;
}

describe('trackPaneSwipe', () => {
  function setup(env: Partial<PaneSwipeEnv> = {}) {
    const surface = new EventTarget();
    const onDrag = vi.fn();
    const onRelease = vi.fn();
    const cleanup = trackPaneSwipe(surface, { onDrag, onRelease }, {
      paneWidth: () => 300,
      textFieldFocused: () => false,
      styleOf,
      ...env,
    });
    const plain = chain({ localName: 'div' });
    const start = (p: [number, number], target: DragNode = plain) =>
      surface.dispatchEvent(touch('touchstart', [p], { target }));
    const move = (p: [number, number], opts: { prevented?: boolean; cancelable?: boolean } = {}) => {
      const e = touch('touchmove', [p], opts);
      surface.dispatchEvent(e);
      return e;
    };
    const release = () => surface.dispatchEvent(touch('touchend', []));
    return { surface, onDrag, onRelease, cleanup, start, move, release };
  }

  it('posts the drag and a far release as a pane change', () => {
    const t = setup();
    t.start([200, 100]);
    t.move([190, 100]);
    t.move([50, 102]);
    expect(t.onDrag).toHaveBeenLastCalledWith(-142);
    t.release();
    expect(t.onRelease).toHaveBeenCalledExactlyOnceWith(1);
  });

  it('follows the finger while the host moves the frame with it', () => {
    // The frame rides the pane track, so after the host applies a drag the
    // frame's own client coordinates lag the finger by that much.
    const t = setup();
    t.start([200, 100]);
    t.surface.dispatchEvent(touch('touchmove', [[150, 100]]));
    expect(t.onDrag).toHaveBeenLastCalledWith(-42);
    t.surface.dispatchEvent(touch('touchmove', [[100, 100]], { frameShift: -42 }));
    expect(t.onDrag).toHaveBeenLastCalledWith(-92);
    t.release();
    expect(t.onRelease).toHaveBeenCalledExactlyOnceWith(1);
  });

  it('snaps back when the frame unloads mid-drag', () => {
    const t = setup();
    t.start([200, 100]);
    t.move([100, 100]);
    t.surface.dispatchEvent(new Event('pagehide'));
    expect(t.onRelease).toHaveBeenCalledExactlyOnceWith(0);
  });

  it('cancels each sideways move so the page cannot drift under it', () => {
    const t = setup();
    t.start([200, 100]);
    expect(t.move([180, 100]).defaultPrevented).toBe(true);
  });

  it('never touches a vertical scroll', () => {
    const t = setup();
    t.start([100, 300]);
    const moves = [t.move([102, 280]), t.move([104, 100])];
    t.release();
    for (const e of moves) expect(e.defaultPrevented).toBe(false);
    expect(t.onDrag).not.toHaveBeenCalled();
    expect(t.onRelease).not.toHaveBeenCalled();
  });

  it('gives the drag to a native pan the browser already committed', () => {
    const t = setup();
    t.start([200, 100]);
    t.move([180, 100]);
    t.move([150, 100], { cancelable: false });
    expect(t.onRelease).toHaveBeenCalledExactlyOnceWith(0);
    t.move([50, 100]);
    t.release();
    expect(t.onDrag).toHaveBeenCalledTimes(1);
    expect(t.onRelease).toHaveBeenCalledTimes(1);
  });

  it('posts nothing for a drag the app claimed', () => {
    const t = setup();
    t.start([200, 100], chain({ localName: 'input', type: 'range' }));
    t.move([100, 100]);
    t.release();
    expect(t.onDrag).not.toHaveBeenCalled();
    expect(t.onRelease).not.toHaveBeenCalled();
  });

  it('posts nothing while a text field has focus', () => {
    const t = setup({ textFieldFocused: () => true });
    t.start([200, 100]);
    t.move([50, 100]);
    t.release();
    expect(t.onDrag).not.toHaveBeenCalled();
  });

  it('posts nothing for a touchstart the app prevented', () => {
    const t = setup();
    t.surface.dispatchEvent(touch('touchstart', [[200, 100]], { target: chain({}), prevented: true }));
    t.move([50, 100]);
    expect(t.onDrag).not.toHaveBeenCalled();
  });

  it('snaps back when the app claims the drag midway', () => {
    const t = setup();
    t.start([200, 100]);
    t.move([150, 100]);
    t.move([100, 100], { prevented: true });
    expect(t.onRelease).toHaveBeenCalledExactlyOnceWith(0);
    t.release();
    expect(t.onRelease).toHaveBeenCalledTimes(1);
  });

  it('snaps back when a second finger lands', () => {
    const t = setup();
    t.start([200, 100]);
    t.move([150, 100]);
    t.surface.dispatchEvent(touch('touchmove', [[150, 100], [250, 100]]));
    expect(t.onRelease).toHaveBeenCalledExactlyOnceWith(0);
  });

  it('snaps back on touchcancel', () => {
    const t = setup();
    t.start([200, 100]);
    t.move([100, 100]);
    t.surface.dispatchEvent(touch('touchcancel', []));
    expect(t.onRelease).toHaveBeenCalledExactlyOnceWith(0);
  });

  it('ends a drag whose touchend never arrived when the next touch starts', () => {
    const t = setup();
    t.start([200, 100]);
    t.move([100, 100]);
    t.start([200, 100]);
    expect(t.onRelease).toHaveBeenCalledExactlyOnceWith(0);
  });

  it('stops listening after cleanup', () => {
    const t = setup();
    t.cleanup();
    t.start([200, 100]);
    t.move([50, 100]);
    t.release();
    expect(t.onDrag).not.toHaveBeenCalled();
    expect(t.onRelease).not.toHaveBeenCalled();
  });
});

describe('installAppPaneSwipe', () => {
  let cleanup: () => void = () => {};

  afterEach(() => {
    cleanup();
    cleanup = () => {};
    delete (window as unknown as { parent?: unknown }).parent;
  });

  it('posts the drag and the release to the host', () => {
    const postMessage = vi.fn();
    (window as unknown as { parent: unknown }).parent = { postMessage };
    cleanup = installAppPaneSwipe();
    window.dispatchEvent(touch('touchstart', [[200, 100]], { target: chain({}) }));
    window.dispatchEvent(touch('touchmove', [[180, 100]]));
    window.dispatchEvent(touch('touchend', []));
    expect(postMessage).toHaveBeenCalledWith({ type: APP_SWIPE_MESSAGE_TYPE, dx: -12 }, '*');
    expect(postMessage).toHaveBeenLastCalledWith({ type: APP_SWIPE_END_MESSAGE_TYPE, paneDelta: 0 }, '*');
  });

  it('does nothing at the top level, where there is no host', () => {
    (window as unknown as { parent: unknown }).parent = window;
    const add = vi.spyOn(window, 'addEventListener');
    cleanup = installAppPaneSwipe();
    expect(add).not.toHaveBeenCalled();
    add.mockRestore();
  });
});
