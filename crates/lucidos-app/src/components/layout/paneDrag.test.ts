import { describe, it, expect, vi } from 'vitest';
import {
  PaneDrag,
  attachPaneDragTouches,
  isFrameInside,
  type PaneDragEnv,
  type PaneTrack,
  type TouchPoint,
} from './paneDrag';
import type { FrameSource } from '../../store/actions/frame-swipe-bridge';

const WIDTH = 300;

function fakeTrack() {
  const calls: string[] = [];
  const state = { held: false };
  const track: PaneTrack = {
    paneWidth: () => WIDTH,
    grab: () => { calls.push('grab'); state.held = true; },
    follow: (dx) => {
      if (!state.held) throw new Error(`follow ${dx} with nothing grabbed`);
      calls.push(`follow ${dx}`);
    },
    navigate: (d) => { calls.push(`navigate ${d}`); },
    rest: (animate) => { calls.push(`rest ${animate}`); state.held = false; },
  };
  return { track, calls, state };
}

function setup(env: Partial<PaneDragEnv> = {}) {
  const t = fakeTrack();
  const drag = new PaneDrag(t.track, { frameMayLead: () => true, mayCommit: () => true, ...env });
  return { ...t, drag };
}

const pt = (identifier: number, clientX: number, clientY = 100): TouchPoint => ({ identifier, clientX, clientY });

const APP = { name: 'app frame' } as unknown as FrameSource;
const PREVIEW = { name: 'preview frame' } as unknown as FrameSource;

function farDrag(s: ReturnType<typeof setup>) {
  s.drag.touchStart(pt(1, 200), [pt(1, 200)], true);
  s.drag.touchMove([pt(1, 180)], true);
  s.drag.touchMove([pt(1, 50)], true);
}

describe('PaneDrag: a host touch', () => {
  it('grabs at the lock, follows, and commits on its lift', () => {
    const s = setup();
    farDrag(s);
    s.drag.touchEnd([pt(1, 50)], []);
    expect(s.calls).toEqual(['grab', 'follow -12', 'follow -142', 'navigate 1', 'rest true']);
    expect(s.drag.holding).toBe(false);
  });

  it('leaves the strip alone for a tap or a vertical scroll', () => {
    const s = setup();
    s.drag.touchStart(pt(1, 200), [pt(1, 200)], true);
    s.drag.touchEnd([pt(1, 200)], []);
    s.drag.touchStart(pt(2, 200), [pt(2, 200)], true);
    expect(s.drag.touchMove([pt(2, 202, 160)], true)).toBe(false);
    s.drag.touchEnd([pt(2, 202, 300)], []);
    expect(s.calls).toEqual([]);
  });

  it('springs back on a cancel, however far it went', () => {
    const s = setup();
    farDrag(s);
    s.drag.touchCancel([pt(1, 50)], []);
    expect(s.calls.slice(-1)).toEqual(['rest true']);
    expect(s.calls).not.toContain('navigate 1');
  });

  it('springs back on an uncancelable move, since the browser owns the pan', () => {
    const s = setup();
    farDrag(s);
    expect(s.drag.touchMove([pt(1, 40)], false)).toBe(false);
    expect(s.calls.slice(-1)).toEqual(['rest true']);
    s.drag.touchEnd([pt(1, 40)], []);
    expect(s.calls).not.toContain('navigate 1');
  });

  it('springs back when the browser no longer lists it', () => {
    const s = setup();
    farDrag(s);
    s.drag.touchMove([pt(2, 10)], true);
    expect(s.calls.slice(-1)).toEqual(['rest true']);
    expect(s.drag.holding).toBe(false);
  });

  it('springs back when another touch ends and the lead is gone from the list', () => {
    const s = setup();
    farDrag(s);
    s.drag.touchEnd([pt(2, 10)], []);
    expect(s.calls).not.toContain('navigate 1');
    expect(s.calls.slice(-1)).toEqual(['rest true']);
  });

  it('lets no refused touch move the strip, even after a lost touchend', () => {
    const s = setup();
    farDrag(s);
    s.drag.touchStart(pt(2, 200), [pt(2, 200)], false);
    s.calls.length = 0;
    s.drag.touchMove([pt(2, 20)], true);
    s.drag.touchEnd([pt(2, 20)], []);
    expect(s.calls).toEqual([]);
  });

  it('springs back instead of committing while fullscreen', () => {
    const s = setup({ mayCommit: () => false });
    farDrag(s);
    s.drag.touchEnd([pt(1, 50)], []);
    expect(s.calls.slice(-1)).toEqual(['rest true']);
    expect(s.calls).not.toContain('navigate 1');
  });
});

describe('PaneDrag: a second touch', () => {
  it('never hijacks a lead that has locked', () => {
    const s = setup();
    farDrag(s);
    expect(s.drag.touchStart(pt(2, 10), [pt(1, 50), pt(2, 10)], true)).toBe(false);
    s.drag.touchMove([pt(1, 50), pt(2, 290)], true);
    s.drag.touchEnd([pt(2, 290)], [pt(1, 50)]);
    s.drag.touchEnd([pt(1, 50)], []);
    expect(s.calls.filter((c) => c.startsWith('navigate'))).toEqual(['navigate 1']);
    expect(s.calls).toContain('follow -142');
    expect(s.calls.every((c) => c !== 'follow 272')).toBe(true);
  });

  it('replaces a resting thumb that has not locked, so the new swipe leads', () => {
    const s = setup();
    s.drag.touchStart(pt(1, 5), [pt(1, 5)], true);
    expect(s.drag.touchStart(pt(2, 200), [pt(1, 5), pt(2, 200)], true)).toBe(true);
    s.drag.touchMove([pt(1, 5), pt(2, 50)], true);
    s.drag.touchEnd([pt(2, 50)], [pt(1, 5)]);
    expect(s.calls).toEqual(['grab', 'follow -142', 'navigate 1', 'rest true']);
  });

  it('ignores the moves and lift of a touch that is not the lead', () => {
    const s = setup();
    farDrag(s);
    s.drag.touchStart(pt(2, 100), [pt(1, 50), pt(2, 100)], true);
    s.calls.length = 0;
    s.drag.touchEnd([pt(2, 100)], [pt(1, 50)]);
    expect(s.calls).toEqual([]);
    expect(s.drag.holding).toBe(true);
  });
});

describe('PaneDrag: frames', () => {
  it('follows a frame and commits on its own release', () => {
    const s = setup();
    s.drag.frameDrag(APP, -20);
    s.drag.frameDrag(APP, -150);
    s.drag.frameRelease(APP, 1);
    expect(s.calls).toEqual(['grab', 'follow -20', 'follow -150', 'navigate 1', 'rest true']);
  });

  it('ignores a release and a drag from another frame', () => {
    const s = setup();
    s.drag.frameDrag(APP, -20);
    s.drag.frameRelease(PREVIEW, 1);
    s.drag.frameDrag(PREVIEW, -200);
    expect(s.calls).toEqual(['grab', 'follow -20']);
    s.drag.frameRelease(PREVIEW, 1);
    s.drag.frameRelease(APP, 0);
    expect(s.calls).toEqual(['grab', 'follow -20', 'rest true']);
  });

  it('springs back when the frame leading goes away', () => {
    const s = setup();
    s.drag.frameDrag(PREVIEW, -120);
    s.drag.frameGone(PREVIEW);
    expect(s.calls).toEqual(['grab', 'follow -120', 'rest true']);
    s.drag.frameDrag(PREVIEW, -10);
    expect(s.calls.slice(-1)).toEqual(['follow -10']);
  });

  it('forgets an ignored frame that went away, so its next gesture may lead', () => {
    const s = setup();
    farDrag(s);
    s.drag.frameDrag(APP, -10);
    s.drag.frameGone(APP);
    s.drag.touchEnd([pt(1, 50)], []);
    s.calls.length = 0;
    s.drag.frameDrag(APP, -30);
    expect(s.calls).toEqual(['grab', 'follow -30']);
  });

  it('springs back a frame drag when a host touch starts, even a refused one', () => {
    const s = setup();
    s.drag.frameDrag(APP, -120);
    s.drag.touchStart(pt(1, 100), [pt(1, 100)], false);
    expect(s.calls).toEqual(['grab', 'follow -120', 'rest true']);
    s.drag.frameDrag(APP, -200);
    s.drag.frameRelease(APP, 1);
    expect(s.calls).toEqual(['grab', 'follow -120', 'rest true']);
    s.drag.frameDrag(APP, -5);
    expect(s.calls.slice(-1)).toEqual(['follow -5']);
  });

  it('ignores a frame drag while a host touch holds the strip', () => {
    const s = setup();
    farDrag(s);
    s.drag.frameDrag(APP, -200);
    s.drag.frameRelease(APP, 1);
    expect(s.calls).toEqual(['grab', 'follow -12', 'follow -142']);
    expect(s.drag.holding).toBe(true);
  });

  it('gates each frame gesture once, and a refused one never moves the strip', () => {
    const frameMayLead = vi.fn(() => false);
    const s = setup({ frameMayLead });
    s.drag.frameDrag(APP, -20);
    s.drag.frameDrag(APP, -200);
    s.drag.frameRelease(APP, 1);
    expect(s.calls).toEqual([]);
    expect(frameMayLead).toHaveBeenCalledTimes(1);
    frameMayLead.mockReturnValue(true);
    s.drag.frameDrag(APP, -20);
    expect(frameMayLead).toHaveBeenCalledTimes(2);
    expect(s.calls).toEqual(['grab', 'follow -20']);
  });

  it('springs back instead of committing a frame release while fullscreen', () => {
    const s = setup({ mayCommit: () => false });
    s.drag.frameDrag(APP, -200);
    s.drag.frameRelease(APP, 1);
    expect(s.calls).toEqual(['grab', 'follow -200', 'rest true']);
  });
});

describe('PaneDrag: an interrupt', () => {
  it('ends a touch drag, and the rest of that gesture does nothing', () => {
    const s = setup();
    farDrag(s);
    s.drag.interrupt(true);
    expect(s.drag.holding).toBe(false);
    s.calls.length = 0;
    s.drag.touchMove([pt(1, 10)], true);
    s.drag.touchEnd([pt(1, 10)], []);
    expect(s.calls).toEqual([]);
  });

  it('ignores the interrupted frame until its own release', () => {
    const s = setup();
    s.drag.frameDrag(APP, -50);
    s.drag.interrupt(false);
    s.drag.frameDrag(APP, -80);
    s.drag.frameRelease(APP, 1);
    expect(s.calls).toEqual(['grab', 'follow -50', 'rest false']);
    s.drag.frameDrag(APP, -5);
    expect(s.calls.slice(-1)).toEqual(['follow -5']);
  });

  it('rests the strip even with no drag', () => {
    const s = setup();
    s.drag.interrupt(false);
    expect(s.calls).toEqual(['rest false']);
  });
});

describe('isFrameInside', () => {
  const host = (...windows: unknown[]) => ({
    querySelectorAll: () => windows.map((contentWindow) => ({ contentWindow })),
  });

  it('accepts a frame inside the strip', () => {
    expect(isFrameInside(host(PREVIEW, APP), APP)).toBe(true);
  });

  it('refuses a frame outside it', () => {
    expect(isFrameInside(host(PREVIEW), APP)).toBe(false);
    expect(isFrameInside(null, APP)).toBe(false);
    expect(isFrameInside(host(null), null)).toBe(false);
  });
});

function seeded(seed: number) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

describe('PaneDrag: random event sequences', () => {
  it('keeps every invariant after every step', () => {
    let commits = 0;
    for (let seed = 1; seed <= 200; seed++) {
      const rand = seeded(seed);
      const pick = <T,>(xs: T[]): T => xs[Math.floor(rand() * xs.length)];
      const { track, calls, state } = fakeTrack();
      let fullscreen = false;
      const drag = new PaneDrag(track, {
        frameMayLead: () => !fullscreen && rand() > 0.2,
        mayCommit: () => !fullscreen,
      });
      const down = new Map<number, TouchPoint>();
      const listed = () => [...down.values()];

      for (let step = 0; step < 120; step++) {
        const before = calls.length;
        const id = pick([1, 2, 3]);
        const frame = pick([APP, PREVIEW]);
        const x = Math.round(rand() * WIDTH);
        const y = Math.round(rand() * 400);
        const action = pick(['start', 'move', 'move', 'move', 'end', 'cancel', 'lose', 'drag', 'drag', 'release', 'gone', 'interrupt', 'fullscreen']);
        let lift = false;
        const leadBefore = drag.leadTouch !== null ? `touch ${drag.leadTouch}` : null;

        switch (action) {
          case 'start':
            if (down.has(id)) break;
            down.set(id, pt(id, x, y));
            drag.touchStart(pt(id, x, y), listed(), rand() > 0.3);
            break;
          case 'move':
            if (!down.has(id)) break;
            down.set(id, pt(id, x, y));
            drag.touchMove(listed(), rand() > 0.1);
            break;
          case 'end':
            if (!down.has(id)) break;
            down.delete(id);
            lift = drag.leadTouch === id;
            drag.touchEnd([pt(id, x, y)], listed());
            break;
          case 'cancel':
            if (!down.has(id)) break;
            down.delete(id);
            drag.touchCancel([pt(id, x, y)], listed());
            break;
          case 'lose':
            down.delete(id);
            break;
          case 'drag':
            drag.frameDrag(frame, x - WIDTH / 2);
            break;
          case 'release':
            lift = true;
            drag.frameRelease(frame, pick([-1, 0, 1]));
            break;
          case 'gone':
            drag.frameGone(frame);
            break;
          case 'interrupt':
            drag.interrupt(rand() > 0.5);
            break;
          case 'fullscreen':
            fullscreen = !fullscreen;
            drag.interrupt(false);
            break;
        }

        const made = calls.slice(before);
        expect(state.held, `seed ${seed} step ${step} ${action}`).toBe(drag.holding);
        const nav = made.findIndex((c) => c.startsWith('navigate'));
        if (nav !== -1) {
          expect(lift, `seed ${seed} step ${step}: ${action} navigated`).toBe(true);
          expect(fullscreen).toBe(false);
          expect(made[nav + 1]).toBe('rest true');
        }
        if (made.some((c) => c.startsWith('follow'))) {
          expect(['move', 'drag'], `seed ${seed} step ${step}`).toContain(action);
          if (action === 'move') expect(leadBefore).toBe(`touch ${drag.leadTouch}`);
        }
        commits += made.filter((c) => c.startsWith('navigate')).length;
      }
    }
    expect(commits).toBeGreaterThan(20);
  });
});

function touchEvent(
  type: string,
  touches: TouchPoint[],
  changed: TouchPoint[],
  opts: { trusted?: boolean; target?: EventTarget; cancelable?: boolean } = {},
): Event {
  const e = new Event(type, { cancelable: opts.cancelable ?? true });
  Object.defineProperty(e, 'isTrusted', { value: opts.trusted ?? false });
  Object.defineProperty(e, 'touches', { value: touches });
  Object.defineProperty(e, 'changedTouches', { value: changed });
  if (opts.target) Object.defineProperty(e, 'target', { value: opts.target });
  return e;
}

describe('attachPaneDragTouches', () => {
  function wire(gate = vi.fn((_e: TouchEvent, _t: TouchPoint) => true)) {
    const s = setup();
    const surface = new EventTarget();
    const cleanup = attachPaneDragTouches(surface, s.drag, { gate });
    return { ...s, surface, cleanup, gate };
  }

  it('drives the drag from synthetic touches, which tests rely on', () => {
    const w = wire();
    w.surface.dispatchEvent(touchEvent('touchstart', [pt(7, 200)], [pt(7, 200)]));
    const move = touchEvent('touchmove', [pt(7, 50)], [pt(7, 50)]);
    w.surface.dispatchEvent(move);
    w.surface.dispatchEvent(touchEvent('touchend', [], [pt(7, 50)]));
    expect(move.defaultPrevented).toBe(true);
    expect(w.calls).toEqual(['grab', 'follow -142', 'navigate 1', 'rest true']);
  });

  it('listens to touchstart without passive, so the edge gate can cancel it', () => {
    const { drag } = setup();
    const surface = { addEventListener: vi.fn() };
    attachPaneDragTouches(surface as unknown as EventTarget, drag, { gate: () => true });
    const start = surface.addEventListener.mock.calls.find(([type]) => type === 'touchstart');
    expect(start?.[2]).toEqual({ passive: false });
  });

  it('ignores an event with no touch lists rather than throwing', () => {
    const w = wire();
    w.surface.dispatchEvent(touchEvent('touchstart', [pt(7, 200)], [pt(7, 200)]));
    expect(() => {
      for (const type of ['touchstart', 'touchmove', 'touchend', 'touchcancel']) w.surface.dispatchEvent(new Event(type));
    }).not.toThrow();
    expect(w.drag.leadTouch).toBe(7);
  });

  it('gates the touch that started, not the oldest one down', () => {
    const w = wire();
    w.surface.dispatchEvent(touchEvent('touchstart', [pt(1, 5)], [pt(1, 5)]));
    w.surface.dispatchEvent(touchEvent('touchstart', [pt(1, 5), pt(2, 200)], [pt(2, 200)]));
    expect(w.gate.mock.calls[1][1]).toEqual(pt(2, 200));
  });

  it('runs the gate for a touch that cannot lead, so the edge guard still acts', () => {
    const w = wire();
    w.surface.dispatchEvent(touchEvent('touchstart', [pt(1, 200)], [pt(1, 200)]));
    w.surface.dispatchEvent(touchEvent('touchmove', [pt(1, 50)], [pt(1, 50)]));
    w.surface.dispatchEvent(touchEvent('touchstart', [pt(1, 50), pt(2, 2)], [pt(2, 2)]));
    expect(w.gate).toHaveBeenCalledTimes(2);
    expect(w.drag.leadTouch).toBe(1);
  });

  it('springs back on touchcancel', () => {
    const w = wire();
    w.surface.dispatchEvent(touchEvent('touchstart', [pt(1, 200)], [pt(1, 200)]));
    w.surface.dispatchEvent(touchEvent('touchmove', [pt(1, 50)], [pt(1, 50)]));
    w.surface.dispatchEvent(touchEvent('touchcancel', [], [pt(1, 50)]));
    expect(w.calls).toEqual(['grab', 'follow -142', 'rest true']);
  });

  function strandedRow() {
    const row = new EventTarget();
    const fire = (e: Event) => {
      Object.defineProperty(e, 'composedPath', { value: () => [row] });
      row.dispatchEvent(e);
    };
    return { row, fire };
  }

  it('follows the lead touch onto its node when that node leaves the DOM', () => {
    const w = wire();
    const { row, fire } = strandedRow();
    w.surface.dispatchEvent(touchEvent('touchstart', [pt(1, 200)], [pt(1, 200)], { target: row }));
    fire(touchEvent('touchmove', [pt(1, 50)], [pt(1, 50)], { trusted: true }));
    fire(touchEvent('touchend', [], [pt(1, 50)], { trusted: true }));
    expect(w.calls).toEqual(['grab', 'follow -142', 'navigate 1', 'rest true']);
  });

  it('keeps following a locked lead when a second touch starts', () => {
    const w = wire();
    const { row, fire } = strandedRow();
    w.surface.dispatchEvent(touchEvent('touchstart', [pt(1, 200)], [pt(1, 200)], { target: row }));
    w.surface.dispatchEvent(touchEvent('touchmove', [pt(1, 50)], [pt(1, 50)]));
    w.surface.dispatchEvent(touchEvent('touchstart', [pt(1, 50), pt(2, 100)], [pt(2, 100)]));
    fire(touchEvent('touchend', [pt(2, 100)], [pt(1, 50)], { trusted: true }));
    expect(w.calls.slice(-2)).toEqual(['navigate 1', 'rest true']);
  });

  it('stops listening on the stranded node and the surface after cleanup', () => {
    const w = wire();
    const { row, fire } = strandedRow();
    w.surface.dispatchEvent(touchEvent('touchstart', [pt(1, 200)], [pt(1, 200)], { target: row }));
    w.cleanup();
    fire(touchEvent('touchmove', [pt(1, 50)], [pt(1, 50)], { trusted: true }));
    w.surface.dispatchEvent(touchEvent('touchmove', [pt(1, 50)], [pt(1, 50)]));
    expect(w.calls).toEqual([]);
  });
});
