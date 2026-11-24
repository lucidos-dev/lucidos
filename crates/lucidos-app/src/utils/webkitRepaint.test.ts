import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

let webKitValue = true;
vi.mock('./platform', () => ({ isWebKit: () => webKitValue, isIOS: () => false }));

let pendingEventScroll = false;
let deepLinkResolved = false;
vi.mock('../components/chat/scrollState', () => ({
  hasPendingEventScroll: () => pendingEventScroll,
  deepLinkHasResolved: () => pendingEventScroll && deepLinkResolved,
}));

let userScrolling = false;
vi.mock('./scrollActivity', () => ({ isUserScrolling: () => userScrolling }));

import { forceWebKitRepaint, forceWebKitRepaintBurst, createRepaintThrottle, isRepaintNudging, NUDGE_EVENT_WINDOW_MS, PINNED_SHIFT_PROP, SCROLLER_PINNED_ATTR, repaintNudgeShift, settledScrollTop } from './webkitRepaint';
import { OPEN_REPAINT_BURST_DELAYS_MS } from './repaintBurst';

describe('OPEN_REPAINT_BURST_DELAYS_MS', () => {
  it('starts with an immediate (0ms) attempt', () => {
    expect(OPEN_REPAINT_BURST_DELAYS_MS[0]).toBe(0);
  });

  it('is strictly ascending (no duplicate / out-of-order setTimeout slots)', () => {
    for (let i = 1; i < OPEN_REPAINT_BURST_DELAYS_MS.length; i++) {
      expect(OPEN_REPAINT_BURST_DELAYS_MS[i]).toBeGreaterThan(OPEN_REPAINT_BURST_DELAYS_MS[i - 1]);
    }
  });

  it('extends past 300ms to cover a layer that blanks later under prolonged use', () => {
    expect(Math.max(...OPEN_REPAINT_BURST_DELAYS_MS)).toBeGreaterThanOrEqual(1000);
  });
});

let rafQueue: Array<() => void>;
let rafIdSeq: number;
let canceled: Set<number>;
const origRaf = (globalThis as any).requestAnimationFrame;
const origCancelRaf = (globalThis as any).cancelAnimationFrame;

function flushFrame() {
  const batch = rafQueue;
  rafQueue = [];
  for (const cb of batch) cb();
}

beforeEach(() => {
  webKitValue = true;
  pendingEventScroll = false;
  deepLinkResolved = false;
  userScrolling = false;
  rafQueue = [];
  rafIdSeq = 0;
  canceled = new Set();
  (globalThis as any).requestAnimationFrame = (cb: () => void) => {
    const id = ++rafIdSeq;
    rafQueue.push(() => { if (!canceled.has(id)) cb(); });
    return id;
  };
  (globalThis as any).cancelAnimationFrame = (id: number) => { canceled.add(id); };
});

afterEach(() => {
  (globalThis as any).requestAnimationFrame = origRaf;
  (globalThis as any).cancelAnimationFrame = origCancelRaf;
});

function styleStub(transform = ''): any {
  const props = new Map<string, string>();
  return {
    transform,
    props,
    setProperty(name: string, value: string) { props.set(name, value); },
    removeProperty(name: string) { props.delete(name); },
  };
}

function fakePinnedChild(): any {
  return { style: styleStub(), hasAttribute: (name: string) => name === SCROLLER_PINNED_ATTR };
}

function fakePlainChild(): any {
  return { style: styleStub(), hasAttribute: () => false };
}

function pinnedShiftPx(child: any): number | null {
  const raw = child.style.props.get(PINNED_SHIFT_PROP);
  return raw === undefined ? null : Number(raw.replace('px', ''));
}

function fakeEl(
  transform = '',
  scroll?: { scrollTop: number; scrollHeight: number; clientHeight: number },
  ...pinned: any[]
): any {
  const el: any = { isConnected: true, style: styleStub(transform), offsetReads: 0 };
  el.children = pinned;
  if (scroll) {
    el.scrollTop = scroll.scrollTop;
    el.scrollHeight = scroll.scrollHeight;
    el.clientHeight = scroll.clientHeight;
  }
  Object.defineProperty(el, 'offsetHeight', {
    get() { el.offsetReads++; return 0; },
    configurable: true,
  });
  return el;
}

function translateYPx(transform: string): number {
  const m = /translateY\((-?\d+(?:\.\d+)?)px\)/.exec(transform);
  return m ? Number(m[1]) : 0;
}

function paintedShiftPx(el: any, from: number): number {
  return translateYPx(el.style.transform) - (el.scrollTop - from);
}

describe('forceWebKitRepaint', () => {
  it('toggles a translateZ across two frames then restores the prior transform', () => {
    const el = fakeEl();
    forceWebKitRepaint(el);
    expect(el.style.transform).toBe('');

    flushFrame();
    expect(el.style.transform).toBe('translateZ(0.1px)');

    flushFrame();
    expect(el.style.transform).toBe('');
  });

  it('preserves an existing inline transform', () => {
    const el = fakeEl('rotate(2deg)');
    forceWebKitRepaint(el);
    flushFrame();
    expect(el.style.transform).toBe('rotate(2deg) translateZ(0.1px)');
    flushFrame();
    expect(el.style.transform).toBe('rotate(2deg)');
  });

  it('is a no-op off WebKit', () => {
    webKitValue = false;
    const el = fakeEl();
    const cleanup = forceWebKitRepaint(el);
    expect(cleanup).toBeUndefined();
    flushFrame();
    flushFrame();
    expect(el.style.transform).toBe('');
  });

  it('is a no-op for a detached node', () => {
    const el = fakeEl();
    el.isConnected = false;
    const cleanup = forceWebKitRepaint(el);
    expect(cleanup).toBeUndefined();
  });

  it('skips the restore if the element detaches mid-toggle', () => {
    const el = fakeEl();
    forceWebKitRepaint(el);
    flushFrame();
    expect(el.style.transform).toBe('translateZ(0.1px)');
    el.isConnected = false;
    flushFrame();
    expect(el.style.transform).toBe('translateZ(0.1px)');
  });

  it('coalesces overlapping calls so no stale transform accumulates', () => {
    const el = fakeEl();
    forceWebKitRepaint(el);
    forceWebKitRepaint(el);
    forceWebKitRepaint(el);
    flushFrame();
    expect(el.style.transform).toBe('translateZ(0.1px)');
    flushFrame();
    expect(el.style.transform).toBe('');

    forceWebKitRepaint(el);
    flushFrame();
    expect(el.style.transform).toBe('translateZ(0.1px)');
    flushFrame();
    expect(el.style.transform).toBe('');
  });

  it('recovers when a prior toggle never completes (iOS drops queued frames)', () => {
    const el = fakeEl();
    forceWebKitRepaint(el);
    rafQueue = [];

    forceWebKitRepaint(el);
    flushFrame();
    expect(el.style.transform).toBe('translateZ(0.1px)');
    flushFrame();
    expect(el.style.transform).toBe('');
  });

  it('supersedes a toggle frozen after its first frame without polluting the baseline', () => {
    const el = fakeEl('rotate(1deg)');
    forceWebKitRepaint(el);
    flushFrame();
    expect(el.style.transform).toBe('rotate(1deg) translateZ(0.1px)');
    rafQueue = [];

    forceWebKitRepaint(el);
    flushFrame();
    expect(el.style.transform).toBe('rotate(1deg) translateZ(0.1px)');
    flushFrame();
    expect(el.style.transform).toBe('rotate(1deg)');
  });

  it('cleanup cancels pending frames so no transform is applied', () => {
    const el = fakeEl();
    const cleanup = forceWebKitRepaint(el)!;
    cleanup();
    flushFrame();
    flushFrame();
    expect(el.style.transform).toBe('');
  });


  it('nudges scrollTop by 1px then restores it across two frames (scrollable)', () => {
    const el = fakeEl('', { scrollTop: 500, scrollHeight: 2000, clientHeight: 800 });
    forceWebKitRepaint(el);
    expect(el.scrollTop).toBe(500);

    flushFrame();
    expect(el.scrollTop).toBe(499);

    flushFrame();
    expect(el.scrollTop).toBe(500);
  });

  it('forces a synchronous layout read (offsetHeight) on the nudge frame', () => {
    const el = fakeEl('', { scrollTop: 100, scrollHeight: 2000, clientHeight: 800 });
    forceWebKitRepaint(el);
    expect(el.offsetReads).toBe(0);
    flushFrame();
    expect(el.offsetReads).toBeGreaterThanOrEqual(1);
  });

  it('yields the restore to a concurrent scroll write (never clobbers useScrollMemory / the chevron)', () => {
    const el = fakeEl('', { scrollTop: 500, scrollHeight: 2000, clientHeight: 800 });
    forceWebKitRepaint(el);
    flushFrame();
    expect(el.scrollTop).toBe(499);
    el.scrollTop = 2000;
    flushFrame();
    expect(el.scrollTop).toBe(2000);
  });

  it('nudges from the LIVE position, not a stale call-time baseline (streaming growth)', () => {
    const el = fakeEl('', { scrollTop: 1200, scrollHeight: 2000, clientHeight: 800 });
    forceWebKitRepaint(el);
    el.scrollHeight = 3000;
    el.scrollTop = 2200;
    flushFrame();
    expect(el.scrollTop).toBe(2199);
    flushFrame();
    expect(el.scrollTop).toBe(2200);
  });

  it('nudges DOWN then restores when at the very top (direction-safe, scrollTop 0)', () => {
    const el = fakeEl('', { scrollTop: 0, scrollHeight: 2000, clientHeight: 800 });
    forceWebKitRepaint(el);
    flushFrame();
    expect(el.scrollTop).toBe(1);
    flushFrame();
    expect(el.scrollTop).toBe(0);
  });

  it('does not touch scrollTop on a non-scrollable element (transform-only fallback)', () => {
    const el = fakeEl('', { scrollTop: 0, scrollHeight: 800, clientHeight: 800 });
    forceWebKitRepaint(el);
    flushFrame();
    expect(el.style.transform).toBe('translateZ(0.1px)');
    expect(el.scrollTop).toBe(0);
    flushFrame();
    expect(el.style.transform).toBe('');
    expect(el.scrollTop).toBe(0);
  });

  it('supersedes a dropped scroll restore without drifting the position', () => {
    const el = fakeEl('', { scrollTop: 500, scrollHeight: 2000, clientHeight: 800 });
    forceWebKitRepaint(el);
    flushFrame();
    expect(el.scrollTop).toBe(499);
    rafQueue = [];

    forceWebKitRepaint(el);
    expect(el.scrollTop).toBe(500);
    flushFrame();
    expect(el.scrollTop).toBe(499);
    flushFrame();
    expect(el.scrollTop).toBe(500);
  });

  it('is a no-op off WebKit for a scrollable element (no scrollTop write, no layout read)', () => {
    webKitValue = false;
    const el = fakeEl('', { scrollTop: 500, scrollHeight: 2000, clientHeight: 800 });
    const cleanup = forceWebKitRepaint(el);
    expect(cleanup).toBeUndefined();
    flushFrame();
    flushFrame();
    expect(el.scrollTop).toBe(500);
    expect(el.offsetReads).toBe(0);
  });


  it('cancels the nudge with a matching translateY, so the painted content never moves', () => {
    const el = fakeEl('', { scrollTop: 500, scrollHeight: 2000, clientHeight: 800 });
    forceWebKitRepaint(el);
    flushFrame();
    expect(el.scrollTop).toBe(499);
    expect(paintedShiftPx(el, 500)).toBe(0);
    expect(el.style.transform).toContain('translateZ(0.1px)');

    flushFrame();
    expect(el.scrollTop).toBe(500);
    expect(el.style.transform).toBe('');
  });

  it('cancels a downward nudge too (at scrollTop 0, where the direction flips)', () => {
    const el = fakeEl('', { scrollTop: 0, scrollHeight: 2000, clientHeight: 800 });
    forceWebKitRepaint(el);
    flushFrame();
    expect(el.scrollTop).toBe(1);
    expect(paintedShiftPx(el, 0)).toBe(0);

    flushFrame();
    expect(el.scrollTop).toBe(0);
    expect(el.style.transform).toBe('');
  });

  it('leads with the compensation, so a scaling baseline cannot magnify it', () => {
    const el = fakeEl('scale(2)', { scrollTop: 500, scrollHeight: 2000, clientHeight: 800 });
    forceWebKitRepaint(el);
    flushFrame();
    expect(el.style.transform).toBe('translateY(-1px) scale(2) translateZ(0.1px)');
    expect(paintedShiftPx(el, 500)).toBe(0);

    flushFrame();
    expect(el.style.transform).toBe('scale(2)');
  });

  it('drops the compensation when the restore YIELDS to a concurrent scroll write', () => {
    const el = fakeEl('', { scrollTop: 500, scrollHeight: 2000, clientHeight: 800 });
    forceWebKitRepaint(el);
    flushFrame();
    el.scrollTop = 2000;
    flushFrame();
    expect(el.scrollTop).toBe(2000);
    expect(el.style.transform).toBe('');
  });

  it('leaves no compensation stranded when a supersede undoes a dropped restore', () => {
    const el = fakeEl('', { scrollTop: 500, scrollHeight: 2000, clientHeight: 800 });
    forceWebKitRepaint(el);
    flushFrame();
    expect(translateYPx(el.style.transform)).toBe(-1);
    rafQueue = [];

    forceWebKitRepaint(el);
    expect(el.scrollTop).toBe(500);
    expect(el.style.transform).toBe('');
    flushFrame();
    expect(paintedShiftPx(el, 500)).toBe(0);
  });

  it('leaves no compensation stranded when the cleanup runs mid-toggle', () => {
    const el = fakeEl('', { scrollTop: 500, scrollHeight: 2000, clientHeight: 800 });
    const cleanup = forceWebKitRepaint(el)!;
    flushFrame();
    cleanup();
    expect(el.scrollTop).toBe(500);
    expect(el.style.transform).toBe('');
  });


  it('hands a pinned child exactly the shift the container moved it by', () => {
    const pinned = fakePinnedChild();
    const el = fakeEl('', { scrollTop: 500, scrollHeight: 2000, clientHeight: 800 }, pinned);
    forceWebKitRepaint(el);

    flushFrame();
    expect(pinnedShiftPx(pinned)).toBe(translateYPx(el.style.transform));
    expect(pinnedShiftPx(pinned)).toBe(-1);

    flushFrame();
    expect(pinnedShiftPx(pinned)).toBeNull();
  });

  it('hands the shift to EVERY marked child, and to no unmarked one', () => {
    const first = fakePinnedChild();
    const turn = fakePlainChild();
    const second = fakePinnedChild();
    const el = fakeEl('', { scrollTop: 500, scrollHeight: 2000, clientHeight: 800 }, first, turn, second);
    forceWebKitRepaint(el);

    flushFrame();
    expect(pinnedShiftPx(first)).toBe(-1);
    expect(pinnedShiftPx(second)).toBe(-1);
    expect(pinnedShiftPx(turn)).toBeNull();

    flushFrame();
    expect(pinnedShiftPx(first)).toBeNull();
    expect(pinnedShiftPx(second)).toBeNull();
  });

  it('hands over a downward shift too (at scrollTop 0, where the direction flips)', () => {
    const pinned = fakePinnedChild();
    const el = fakeEl('', { scrollTop: 0, scrollHeight: 2000, clientHeight: 800 }, pinned);
    forceWebKitRepaint(el);

    flushFrame();
    expect(pinnedShiftPx(pinned)).toBe(translateYPx(el.style.transform));
    expect(pinnedShiftPx(pinned)).toBe(1);
  });

  it('writes the counter on the CHILD, never on the scroller', () => {
    const pinned = fakePinnedChild();
    const el = fakeEl('', { scrollTop: 500, scrollHeight: 2000, clientHeight: 800 }, pinned);
    forceWebKitRepaint(el);
    flushFrame();
    expect(el.style.props.has(PINNED_SHIFT_PROP)).toBe(false);
    expect(pinnedShiftPx(pinned)).toBe(-1);
  });

  it('hands over nothing wherever the nudge is skipped', () => {
    const unscrollable = fakePinnedChild();
    forceWebKitRepaint(fakeEl('', { scrollTop: 0, scrollHeight: 800, clientHeight: 800 }, unscrollable));
    flushFrame();
    expect(pinnedShiftPx(unscrollable)).toBeNull();

    pendingEventScroll = true;
    const claimed = fakePinnedChild();
    forceWebKitRepaint(fakeEl('', { scrollTop: 500, scrollHeight: 2000, clientHeight: 800 }, claimed));
    flushFrame();
    expect(pinnedShiftPx(claimed)).toBeNull();
    pendingEventScroll = false;

    userScrolling = true;
    const dragging = fakePinnedChild();
    forceWebKitRepaint(fakeEl('', { scrollTop: 500, scrollHeight: 2000, clientHeight: 800 }, dragging));
    flushFrame();
    expect(pinnedShiftPx(dragging)).toBeNull();
    userScrolling = false;
  });

  it('retires the counter when the restore YIELDS to a concurrent scroll write', () => {
    const pinned = fakePinnedChild();
    const el = fakeEl('', { scrollTop: 500, scrollHeight: 2000, clientHeight: 800 }, pinned);
    forceWebKitRepaint(el);
    flushFrame();
    el.scrollTop = 2000;
    flushFrame();
    expect(el.style.transform).toBe('');
    expect(pinnedShiftPx(pinned)).toBeNull();
  });

  it('retires the counter when a supersede undoes a dropped restore', () => {
    const pinned = fakePinnedChild();
    const el = fakeEl('', { scrollTop: 500, scrollHeight: 2000, clientHeight: 800 }, pinned);
    forceWebKitRepaint(el);
    flushFrame();
    expect(pinnedShiftPx(pinned)).toBe(-1);
    rafQueue = [];

    forceWebKitRepaint(el);
    expect(el.style.transform).toBe('');
    expect(pinnedShiftPx(pinned)).toBeNull();
    flushFrame();
    expect(pinnedShiftPx(pinned)).toBe(-1);
  });

  it('retires the counter when the cleanup runs mid-toggle', () => {
    const pinned = fakePinnedChild();
    const el = fakeEl('', { scrollTop: 500, scrollHeight: 2000, clientHeight: 800 }, pinned);
    const cleanup = forceWebKitRepaint(el)!;
    flushFrame();
    cleanup();
    expect(pinnedShiftPx(pinned)).toBeNull();
  });


  it('skips the scrollTop nudge while a deep-link claim is held, but still repaints (transform + layout read)', () => {
    pendingEventScroll = true;
    const el = fakeEl('', { scrollTop: 500, scrollHeight: 2000, clientHeight: 800 });
    forceWebKitRepaint(el);

    flushFrame();
    expect(el.scrollTop).toBe(500);
    expect(el.style.transform).toBe('translateZ(0.1px)');
    expect(el.offsetReads).toBeGreaterThanOrEqual(1);

    flushFrame();
    expect(el.scrollTop).toBe(500);
    expect(el.style.transform).toBe('');
  });

  it('nudges once the deep link has LANDED, though its claim is still held', () => {
    pendingEventScroll = true;
    deepLinkResolved = true;
    const el = fakeEl('', { scrollTop: 500, scrollHeight: 2000, clientHeight: 800 });
    forceWebKitRepaint(el);

    flushFrame();
    expect(el.scrollTop).toBe(499);
    flushFrame();
    expect(el.scrollTop).toBe(500);
  });

  it('still nudges + restores scrollTop when NO claim is held (repaint not regressed)', () => {
    pendingEventScroll = false;
    const el = fakeEl('', { scrollTop: 500, scrollHeight: 2000, clientHeight: 800 });
    forceWebKitRepaint(el);

    flushFrame();
    expect(el.scrollTop).toBe(499);
    flushFrame();
    expect(el.scrollTop).toBe(500);
  });

  it('keeps a consistent nudge decision when the claim ARRIVES mid-burst (no off-baseline strand)', () => {
    pendingEventScroll = false;
    const el = fakeEl('', { scrollTop: 500, scrollHeight: 2000, clientHeight: 800 });
    forceWebKitRepaint(el);

    flushFrame();
    expect(el.scrollTop).toBe(499);
    pendingEventScroll = true;
    flushFrame();
    expect(el.scrollTop).toBe(500);
  });

  it('keeps a consistent skip decision when the claim RELEASES mid-burst (never nudges)', () => {
    pendingEventScroll = true;
    const el = fakeEl('', { scrollTop: 500, scrollHeight: 2000, clientHeight: 800 });
    forceWebKitRepaint(el);

    flushFrame();
    expect(el.scrollTop).toBe(500);
    pendingEventScroll = false;
    flushFrame();
    expect(el.scrollTop).toBe(500);
  });

  it('reuses the burst nudge decision across a superseding call (claim held → no nudge)', () => {
    pendingEventScroll = true;
    const el = fakeEl('', { scrollTop: 500, scrollHeight: 2000, clientHeight: 800 });
    forceWebKitRepaint(el);
    forceWebKitRepaint(el);
    forceWebKitRepaint(el);

    flushFrame();
    expect(el.scrollTop).toBe(500);
    expect(el.style.transform).toBe('translateZ(0.1px)');
    flushFrame();
    expect(el.scrollTop).toBe(500);
    expect(el.style.transform).toBe('');
  });


  it('skips the scrollTop nudge while the user is actively scrolling, but still repaints (transform + layout read)', () => {
    userScrolling = true;
    const el = fakeEl('', { scrollTop: 500, scrollHeight: 2000, clientHeight: 800 });
    forceWebKitRepaint(el);

    flushFrame();
    expect(el.scrollTop).toBe(500);
    expect(el.style.transform).toBe('translateZ(0.1px)');
    expect(el.offsetReads).toBeGreaterThanOrEqual(1);

    flushFrame();
    expect(el.scrollTop).toBe(500);
    expect(el.style.transform).toBe('');
  });

  it('re-checks live scrolling at the write frame — a drag that starts after the (idle) call but before rAF1 still skips the nudge', () => {
    userScrolling = false;
    const el = fakeEl('', { scrollTop: 500, scrollHeight: 2000, clientHeight: 800 });
    forceWebKitRepaint(el);
    userScrolling = true;

    flushFrame();
    expect(el.scrollTop).toBe(500);
    expect(el.style.transform).toBe('translateZ(0.1px)');
    expect(el.offsetReads).toBeGreaterThanOrEqual(1);

    flushFrame();
    expect(el.scrollTop).toBe(500);
    expect(el.style.transform).toBe('');
  });

  it('still nudges + restores scrollTop when the user is NOT scrolling (recovery not regressed)', () => {
    userScrolling = false;
    const el = fakeEl('', { scrollTop: 500, scrollHeight: 2000, clientHeight: 800 });
    forceWebKitRepaint(el);
    flushFrame();
    expect(el.scrollTop).toBe(499);
    flushFrame();
    expect(el.scrollTop).toBe(500);
  });

  it('reuses the burst skip decision across a superseding call (scrolling → never nudges)', () => {
    userScrolling = true;
    const el = fakeEl('', { scrollTop: 500, scrollHeight: 2000, clientHeight: 800 });
    forceWebKitRepaint(el);
    forceWebKitRepaint(el);
    forceWebKitRepaint(el);
    flushFrame();
    expect(el.scrollTop).toBe(500);
    expect(el.style.transform).toBe('translateZ(0.1px)');
    flushFrame();
    expect(el.scrollTop).toBe(500);
    expect(el.style.transform).toBe('');
  });

  it('keeps a consistent skip decision when scrolling ends mid-burst (never nudges)', () => {
    userScrolling = true;
    const el = fakeEl('', { scrollTop: 500, scrollHeight: 2000, clientHeight: 800 });
    forceWebKitRepaint(el);
    flushFrame();
    expect(el.scrollTop).toBe(500);
    userScrolling = false;
    flushFrame();
    expect(el.scrollTop).toBe(500);
  });
});

describe('isRepaintNudging', () => {
  function nudgedSince(since: number): boolean {
    return isRepaintNudging(since + NUDGE_EVENT_WINDOW_MS);
  }

  it('reports nudging from the nudge write until after the restore', () => {
    const el = fakeEl('', { scrollTop: 500, scrollHeight: 2000, clientHeight: 800 });
    const t0 = performance.now();
    forceWebKitRepaint(el);

    expect(nudgedSince(t0)).toBe(false);

    flushFrame();
    expect(el.scrollTop).toBe(499);
    const afterNudge = performance.now();
    expect(nudgedSince(t0)).toBe(true);

    flushFrame();
    expect(el.scrollTop).toBe(500);
    expect(nudgedSince(afterNudge)).toBe(true);
  });

  it('closes the window once it has elapsed, so a real scroll is never swallowed', () => {
    const el = fakeEl('', { scrollTop: 500, scrollHeight: 2000, clientHeight: 800 });
    forceWebKitRepaint(el);
    flushFrame();
    const atNudge = performance.now();
    expect(isRepaintNudging(atNudge + NUDGE_EVENT_WINDOW_MS)).toBe(false);
  });

  it('stays closed for the transform-only repaint (no scrollTop write)', () => {
    const el = fakeEl();
    const t0 = performance.now();
    forceWebKitRepaint(el);
    flushFrame();
    flushFrame();
    expect(nudgedSince(t0)).toBe(false);
  });

  it('stays closed when the nudge is skipped for an in-flight user scroll', () => {
    userScrolling = true;
    const el = fakeEl('', { scrollTop: 500, scrollHeight: 2000, clientHeight: 800 });
    const t0 = performance.now();
    forceWebKitRepaint(el);
    flushFrame();
    flushFrame();
    expect(nudgedSince(t0)).toBe(false);
  });
});

describe('settledScrollTop', () => {

  it('reads through the nudge while it is in its nudged frame', () => {
    const el = fakeEl('', { scrollTop: 500, scrollHeight: 2000, clientHeight: 800 });
    forceWebKitRepaint(el);
    expect(settledScrollTop(el)).toBe(500);

    flushFrame();
    expect(el.scrollTop).toBe(499);
    expect(repaintNudgeShift(el)).toBe(-1);
    expect(settledScrollTop(el)).toBe(500);

    flushFrame();
    expect(repaintNudgeShift(el)).toBe(0);
    expect(settledScrollTop(el)).toBe(500);
  });

  it('reads through the +1px nudge at the very top too', () => {
    const el = fakeEl('', { scrollTop: 0, scrollHeight: 2000, clientHeight: 800 });
    forceWebKitRepaint(el);
    flushFrame();
    expect(el.scrollTop).toBe(1);
    expect(settledScrollTop(el)).toBe(0);
  });

  it('believes another writer that moved the container mid-nudge', () => {
    const el = fakeEl('', { scrollTop: 500, scrollHeight: 2000, clientHeight: 800 });
    forceWebKitRepaint(el);
    flushFrame();
    el.scrollTop = 1337;
    expect(settledScrollTop(el)).toBe(1337);
    flushFrame();
    expect(el.scrollTop).toBe(1337);
  });

  it('is the plain scrollTop for an element nobody is nudging', () => {
    const el = fakeEl('', { scrollTop: 420, scrollHeight: 2000, clientHeight: 800 });
    expect(settledScrollTop(el)).toBe(420);
  });
});

describe('forceWebKitRepaintBurst', () => {
  beforeEach(() => { vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] }); });
  afterEach(() => { vi.useRealTimers(); });

  it('fires an immediate toggle then setTimeout-spaced retries', () => {
    const el = fakeEl();
    forceWebKitRepaintBurst(el);

    expect(el.style.transform).toBe('');
    flushFrame();
    expect(el.style.transform).toBe('translateZ(0.1px)');
    flushFrame();
    expect(el.style.transform).toBe('');

    vi.advanceTimersByTime(100);
    flushFrame();
    expect(el.style.transform).toBe('translateZ(0.1px)');
    flushFrame();
    expect(el.style.transform).toBe('');

    vi.advanceTimersByTime(200);
    flushFrame();
    expect(el.style.transform).toBe('translateZ(0.1px)');
    flushFrame();
    expect(el.style.transform).toBe('');
  });

  it('recovers when the immediate toggle frames are dropped (iOS coalesces) via a later retry', () => {
    const el = fakeEl();
    forceWebKitRepaintBurst(el);
    rafQueue = [];
    expect(el.style.transform).toBe('');

    vi.advanceTimersByTime(100);
    flushFrame();
    expect(el.style.transform).toBe('translateZ(0.1px)');
    flushFrame();
    expect(el.style.transform).toBe('');
  });

  it('preserves an existing inline transform across the burst', () => {
    const el = fakeEl('rotate(2deg)');
    forceWebKitRepaintBurst(el);
    flushFrame();
    expect(el.style.transform).toBe('rotate(2deg) translateZ(0.1px)');
    flushFrame();
    expect(el.style.transform).toBe('rotate(2deg)');

    vi.advanceTimersByTime(300);
    flushFrame();
    flushFrame();
    expect(el.style.transform).toBe('rotate(2deg)');
  });

  it('is a no-op off WebKit', () => {
    webKitValue = false;
    const el = fakeEl();
    const cleanup = forceWebKitRepaintBurst(el);
    expect(cleanup).toBeUndefined();
    vi.advanceTimersByTime(1000);
    flushFrame();
    flushFrame();
    expect(el.style.transform).toBe('');
  });

  it('is a no-op for a detached node', () => {
    const el = fakeEl();
    el.isConnected = false;
    const cleanup = forceWebKitRepaintBurst(el);
    expect(cleanup).toBeUndefined();
  });

  it('cleanup cancels the immediate frames and all pending retries', () => {
    const el = fakeEl();
    const cleanup = forceWebKitRepaintBurst(el)!;
    cleanup();
    flushFrame();
    flushFrame();
    expect(el.style.transform).toBe('');
    vi.advanceTimersByTime(1000);
    flushFrame();
    flushFrame();
    expect(el.style.transform).toBe('');
  });
});

describe('createRepaintThrottle', () => {
  beforeEach(() => { vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] }); });
  afterEach(() => { vi.useRealTimers(); });

  it('fires immediately on the leading edge', () => {
    const fire = vi.fn();
    const gate = createRepaintThrottle(200);
    gate.request(0, fire);
    expect(fire).toHaveBeenCalledTimes(1);
  });

  it('throttles a leading repaint but fires the request on the trailing edge', () => {
    const fire = vi.fn();
    const gate = createRepaintThrottle(200);
    gate.request(1000, fire);
    expect(fire).toHaveBeenCalledTimes(1);
    gate.request(1100, fire);
    gate.request(1199, fire);
    expect(fire).toHaveBeenCalledTimes(1);
    vi.advanceTimersByTime(1);
    expect(fire).toHaveBeenCalledTimes(2);
    gate.request(1250, fire);
    expect(fire).toHaveBeenCalledTimes(2);
  });

  it('a single request throttled right after a leading repaint still paints once activity stops', () => {
    const fire = vi.fn();
    const gate = createRepaintThrottle(200);
    gate.request(0, fire);
    expect(fire).toHaveBeenCalledTimes(1);
    gate.request(40, fire);
    expect(fire).toHaveBeenCalledTimes(1);
    vi.advanceTimersByTime(200);
    expect(fire).toHaveBeenCalledTimes(2);
  });

  it('coalesces a burst of throttled requests into one trailing fire', () => {
    const fire = vi.fn();
    const gate = createRepaintThrottle(200);
    gate.request(0, fire);
    gate.request(50, fire);
    gate.request(100, fire);
    gate.request(150, fire);
    expect(fire).toHaveBeenCalledTimes(1);
    vi.advanceTimersByTime(50);
    expect(fire).toHaveBeenCalledTimes(2);
    vi.advanceTimersByTime(1000);
    expect(fire).toHaveBeenCalledTimes(2);
  });

  it('measures the window from the last ALLOWED fire, not the last attempt', () => {
    const fire = vi.fn();
    const gate = createRepaintThrottle(200);
    gate.request(0, fire);
    expect(fire).toHaveBeenCalledTimes(1);
    gate.request(150, fire);
    gate.request(199, fire);
    gate.request(200, fire);
    expect(fire).toHaveBeenCalledTimes(2);
  });

  it('fires the leading edge at the interval cadence under a dense stream', () => {
    const fire = vi.fn();
    const gate = createRepaintThrottle(100);
    for (let t = 0; t <= 350; t += 25) gate.request(t, fire);
    expect(fire).toHaveBeenCalledTimes(4);
  });

  it('cancel() clears a pending trailing fire', () => {
    const fire = vi.fn();
    const gate = createRepaintThrottle(200);
    gate.request(0, fire);
    gate.request(40, fire);
    gate.cancel();
    vi.advanceTimersByTime(1000);
    expect(fire).toHaveBeenCalledTimes(1);
  });
});
