import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

// platform.ts touches navigator/window at import time (no jsdom here), so mock
// it outright — the helper only consumes isIOS().
let iosValue = true;
vi.mock('./platform', () => ({ isIOS: () => iosValue }));

import { forceIOSRepaint, createRepaintThrottle } from './iosRepaint';

// Minimal rAF stub: queue callbacks, run them on demand so the two-frame
// transform toggle is deterministic.
let rafQueue: Array<() => void>;
let rafIdSeq: number;
let canceled: Set<number>;
const origRaf = (globalThis as any).requestAnimationFrame;
const origCancelRaf = (globalThis as any).cancelAnimationFrame;

function flushFrame() {
  // Snapshot then clear, so callbacks that schedule the next frame land in the
  // fresh queue rather than running within this flush.
  const batch = rafQueue;
  rafQueue = [];
  for (const cb of batch) cb();
}

beforeEach(() => {
  iosValue = true;
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
  // Restore the originals rather than deleting — leaving these undefined would
  // break any other module that calls rAF in the shared worker context.
  (globalThis as any).requestAnimationFrame = origRaf;
  (globalThis as any).cancelAnimationFrame = origCancelRaf;
});

function fakeEl(transform = ''): any {
  return { isConnected: true, style: { transform } };
}

describe('forceIOSRepaint', () => {
  it('toggles a translateZ across two frames then restores the prior transform', () => {
    const el = fakeEl();
    forceIOSRepaint(el);
    expect(el.style.transform).toBe(''); // nothing yet — deferred to rAF

    flushFrame(); // frame 1: set the nudge
    expect(el.style.transform).toBe('translateZ(0.1px)');

    flushFrame(); // frame 2: restore
    expect(el.style.transform).toBe('');
  });

  it('preserves an existing inline transform', () => {
    const el = fakeEl('rotate(2deg)');
    forceIOSRepaint(el);
    flushFrame();
    expect(el.style.transform).toBe('rotate(2deg) translateZ(0.1px)');
    flushFrame();
    expect(el.style.transform).toBe('rotate(2deg)');
  });

  it('is a no-op off iOS', () => {
    iosValue = false;
    const el = fakeEl();
    const cleanup = forceIOSRepaint(el);
    expect(cleanup).toBeUndefined();
    flushFrame();
    flushFrame();
    expect(el.style.transform).toBe('');
  });

  it('is a no-op for a detached node', () => {
    const el = fakeEl();
    el.isConnected = false;
    const cleanup = forceIOSRepaint(el);
    expect(cleanup).toBeUndefined();
  });

  it('skips the restore if the element detaches mid-toggle', () => {
    const el = fakeEl();
    forceIOSRepaint(el);
    flushFrame(); // frame 1: applies the nudge
    expect(el.style.transform).toBe('translateZ(0.1px)');
    el.isConnected = false;
    flushFrame(); // frame 2: bails, leaves transform as-is (element is gone)
    expect(el.style.transform).toBe('translateZ(0.1px)');
  });

  it('coalesces overlapping calls so no stale transform accumulates', () => {
    // A single iOS resume can fire visibilitychange + pageshow + focus in one
    // tick. Each calls forceIOSRepaint; a superseding call reuses the first
    // call's captured baseline, so the net effect stays one repaint per burst
    // without a growing `translateZ(0.1px) translateZ(0.1px) …` cruft.
    const el = fakeEl();
    forceIOSRepaint(el);
    forceIOSRepaint(el); // in-flight → supersedes, same baseline
    forceIOSRepaint(el); // in-flight → supersedes, same baseline
    flushFrame();
    expect(el.style.transform).toBe('translateZ(0.1px)');
    flushFrame();
    expect(el.style.transform).toBe(''); // restored to the true baseline

    // Once the toggle finishes the element is repaintable again.
    forceIOSRepaint(el);
    flushFrame();
    expect(el.style.transform).toBe('translateZ(0.1px)');
    flushFrame();
    expect(el.style.transform).toBe('');
  });

  it('recovers when a prior toggle never completes (iOS drops queued frames)', () => {
    // iOS suspends a backgrounded PWA and can DROP its queued rAF callbacks
    // rather than deferring them. If the page froze between scheduling the
    // toggle and its second frame, the old "skip while pending" guard left the
    // element permanently locked out: every later repaint — including the
    // resume / open-thread repaint meant to un-blank the layer — no-op'd, so
    // the thread content stayed black until the element was recreated. The
    // up-chevron-but-blank screenshot is this state.
    const el = fakeEl();
    forceIOSRepaint(el); // schedules frame 1, which never runs (page frozen)
    // Simulate the OS dropping the queued frame: clear without running it.
    rafQueue = [];

    // Resume fires another repaint on the SAME element — it must supersede the
    // dropped toggle and actually paint, not skip.
    forceIOSRepaint(el);
    flushFrame();
    expect(el.style.transform).toBe('translateZ(0.1px)');
    flushFrame();
    expect(el.style.transform).toBe(''); // back to the true baseline
  });

  it('supersedes a toggle frozen after its first frame without polluting the baseline', () => {
    // Frame 1 ran (nudge applied) but frame 2 (the restore) was dropped by the
    // suspend. A superseding call must restore the TRUE baseline and toggle
    // again — not read the nudged value as the new baseline and accumulate.
    const el = fakeEl('rotate(1deg)');
    forceIOSRepaint(el);
    flushFrame();
    expect(el.style.transform).toBe('rotate(1deg) translateZ(0.1px)');
    rafQueue = []; // page freezes — the restore frame is dropped

    forceIOSRepaint(el);
    flushFrame();
    expect(el.style.transform).toBe('rotate(1deg) translateZ(0.1px)');
    flushFrame();
    expect(el.style.transform).toBe('rotate(1deg)'); // true baseline, no cruft
  });

  it('cleanup cancels pending frames so no transform is applied', () => {
    const el = fakeEl();
    const cleanup = forceIOSRepaint(el)!;
    cleanup();
    flushFrame();
    flushFrame();
    expect(el.style.transform).toBe('');
  });
});

describe('createRepaintThrottle', () => {
  it('allows the first call immediately', () => {
    const gate = createRepaintThrottle(200);
    expect(gate(0)).toBe(true);
  });

  it('blocks repeat calls inside the interval, allows once it elapses', () => {
    const gate = createRepaintThrottle(200);
    expect(gate(1000)).toBe(true);  // first — fires
    expect(gate(1100)).toBe(false); // +100ms — throttled
    expect(gate(1199)).toBe(false); // +199ms — still throttled
    expect(gate(1200)).toBe(true);  // +200ms — fires again
    expect(gate(1250)).toBe(false); // throttle re-armed from 1200
  });

  it('measures the window from the last ALLOWED call, not the last attempt', () => {
    const gate = createRepaintThrottle(200);
    expect(gate(0)).toBe(true);
    // A burst of blocked attempts must not slide the window forward — otherwise
    // a steady stream of tokens (each <200ms apart) would never repaint.
    expect(gate(150)).toBe(false);
    expect(gate(199)).toBe(false);
    expect(gate(200)).toBe(true); // 200ms since the allowed call at 0, not since 199
  });

  it('keeps firing at the cadence under a dense stream of attempts', () => {
    const gate = createRepaintThrottle(100);
    const allowed: number[] = [];
    for (let t = 0; t <= 350; t += 25) {
      if (gate(t)) allowed.push(t);
    }
    expect(allowed).toEqual([0, 100, 200, 300]);
  });
});
