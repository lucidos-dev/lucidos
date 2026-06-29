import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

// platform.ts touches navigator/window at import time (no jsdom here), so mock
// it outright — the helper only consumes isIOS().
let iosValue = true;
vi.mock('./platform', () => ({ isIOS: () => iosValue }));

import { forceIOSRepaint, forceIOSRepaintBurst, createRepaintThrottle, OPEN_REPAINT_BURST_DELAYS_MS } from './iosRepaint';

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
    // Regression: the old [0,100,300] tail fired its last attempt before a
    // late blank landed on a degraded WKWebView, leaving the body black.
    expect(Math.max(...OPEN_REPAINT_BURST_DELAYS_MS)).toBeGreaterThanOrEqual(1000);
  });
});

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

/** A fake element. Pass `scroll` to make it scrollable (the scroll-nudge path);
 *  omit it for the transform-only cases (a non-scrollable element). `offsetHeight`
 *  is a counting getter so a test can assert the forced synchronous layout read
 *  actually happens. */
function fakeEl(transform = '', scroll?: { scrollTop: number; scrollHeight: number; clientHeight: number }): any {
  const el: any = { isConnected: true, style: { transform }, offsetReads: 0 };
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

  // --- scroll-nudge escalation (the WKCompositingView-removal recovery) -----

  it('nudges scrollTop by 1px then restores it across two frames (scrollable)', () => {
    const el = fakeEl('', { scrollTop: 500, scrollHeight: 2000, clientHeight: 800 });
    forceIOSRepaint(el);
    expect(el.scrollTop).toBe(500); // deferred to rAF — nothing yet

    flushFrame(); // frame 1: nudge up by 1 (re-tiles the frozen layer)
    expect(el.scrollTop).toBe(499);

    flushFrame(); // frame 2: restore (not at bottom → exact prior position)
    expect(el.scrollTop).toBe(500);
  });

  it('forces a synchronous layout read (offsetHeight) on the nudge frame', () => {
    // The documented reliable repaint trigger — a layout read flushes the nudged
    // state so it actually paints instead of being coalesced with the restore.
    const el = fakeEl('', { scrollTop: 100, scrollHeight: 2000, clientHeight: 800 });
    forceIOSRepaint(el);
    expect(el.offsetReads).toBe(0); // deferred to rAF
    flushFrame(); // frame 1 applies the nudge AND reads offsetHeight
    expect(el.offsetReads).toBeGreaterThanOrEqual(1);
  });

  it('yields the restore to a concurrent scroll write (never clobbers useScrollMemory / autoscroll)', () => {
    // The open-path race: useScrollMemory restores a scrolled-up thread to its
    // saved position (or useAutoScroll pins to a new bottom during streaming)
    // BETWEEN the nudge and its restore. The restore must yield — only undo OUR
    // nudge if scrollTop is still the value we left — so it can't snap the user
    // back to a stale position.
    const el = fakeEl('', { scrollTop: 500, scrollHeight: 2000, clientHeight: 800 });
    forceIOSRepaint(el);
    flushFrame(); // frame 1: nudge to 499
    expect(el.scrollTop).toBe(499);
    el.scrollTop = 2000; // a concurrent writer (useScrollMemory restore) moves it
    flushFrame(); // frame 2: 2000 !== 499 → yield, leave it alone
    expect(el.scrollTop).toBe(2000);
  });

  it('nudges DOWN then restores when at the very top (direction-safe, scrollTop 0)', () => {
    const el = fakeEl('', { scrollTop: 0, scrollHeight: 2000, clientHeight: 800 });
    forceIOSRepaint(el);
    flushFrame(); // frame 1: can't go below 0, so nudge +1
    expect(el.scrollTop).toBe(1);
    flushFrame(); // frame 2: restore to 0 (not at bottom)
    expect(el.scrollTop).toBe(0);
  });

  it('does not touch scrollTop on a non-scrollable element (transform-only fallback)', () => {
    const el = fakeEl('', { scrollTop: 0, scrollHeight: 800, clientHeight: 800 });
    forceIOSRepaint(el);
    flushFrame();
    expect(el.style.transform).toBe('translateZ(0.1px)'); // transform still fires
    expect(el.scrollTop).toBe(0); // scroll untouched
    flushFrame();
    expect(el.style.transform).toBe('');
    expect(el.scrollTop).toBe(0);
  });

  it('supersedes a dropped scroll restore without drifting the position', () => {
    // iOS dropped the restore frame; a superseding call must undo the partial
    // nudge to the TRUE baseline (not read the nudged 499 as the new baseline)
    // and round-trip again — no 1px-per-burst drift.
    const el = fakeEl('', { scrollTop: 500, scrollHeight: 2000, clientHeight: 800 });
    forceIOSRepaint(el);
    flushFrame(); // nudge to 499
    expect(el.scrollTop).toBe(499);
    rafQueue = []; // page freezes — the restore frame is dropped

    forceIOSRepaint(el); // supersede — immediately restores the true baseline
    expect(el.scrollTop).toBe(500);
    flushFrame(); // fresh nudge
    expect(el.scrollTop).toBe(499);
    flushFrame(); // fresh restore
    expect(el.scrollTop).toBe(500);
  });

  it('is a no-op off iOS for a scrollable element (no scrollTop write, no layout read)', () => {
    iosValue = false;
    const el = fakeEl('', { scrollTop: 500, scrollHeight: 2000, clientHeight: 800 });
    const cleanup = forceIOSRepaint(el);
    expect(cleanup).toBeUndefined();
    flushFrame();
    flushFrame();
    expect(el.scrollTop).toBe(500);
    expect(el.offsetReads).toBe(0);
  });
});

describe('forceIOSRepaintBurst', () => {
  // The file-level beforeEach already installs the manual rAF stub + iosValue.
  // Add ONLY setTimeout/clearTimeout fakes so the burst's spaced retries are
  // driven deterministically while rAF stays the manual queue.
  beforeEach(() => { vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] }); });
  afterEach(() => { vi.useRealTimers(); });

  it('fires an immediate toggle then setTimeout-spaced retries', () => {
    const el = fakeEl();
    forceIOSRepaintBurst(el);

    // Immediate attempt: deferred to rAF, nothing applied yet.
    expect(el.style.transform).toBe('');
    flushFrame(); // immediate frame 1: nudge
    expect(el.style.transform).toBe('translateZ(0.1px)');
    flushFrame(); // immediate frame 2: restore
    expect(el.style.transform).toBe('');

    // First retry at 100ms fires a fresh, independent toggle.
    vi.advanceTimersByTime(100);
    flushFrame();
    expect(el.style.transform).toBe('translateZ(0.1px)');
    flushFrame();
    expect(el.style.transform).toBe('');

    // Second retry at 300ms (200ms more) fires another.
    vi.advanceTimersByTime(200);
    flushFrame();
    expect(el.style.transform).toBe('translateZ(0.1px)');
    flushFrame();
    expect(el.style.transform).toBe('');
  });

  it('recovers when the immediate toggle frames are dropped (iOS coalesces) via a later retry', () => {
    // The blank-on-open repro: the open path fires ONE toggle and iOS drops its
    // queued rAF callbacks (cold open / suspended frame queue). With no retry the
    // thread stays black (up-chevron visible, content in the DOM) until a manual
    // scroll. The burst's spaced setTimeout retry lands a fresh toggle that
    // actually paints.
    const el = fakeEl();
    forceIOSRepaintBurst(el);
    rafQueue = []; // OS drops the immediate toggle's frames without running them
    expect(el.style.transform).toBe(''); // immediate attempt never painted

    vi.advanceTimersByTime(100); // first retry
    flushFrame();
    expect(el.style.transform).toBe('translateZ(0.1px)'); // recovered
    flushFrame();
    expect(el.style.transform).toBe('');
  });

  it('preserves an existing inline transform across the burst', () => {
    const el = fakeEl('rotate(2deg)');
    forceIOSRepaintBurst(el);
    flushFrame();
    expect(el.style.transform).toBe('rotate(2deg) translateZ(0.1px)');
    flushFrame();
    expect(el.style.transform).toBe('rotate(2deg)');

    vi.advanceTimersByTime(300); // run all remaining retries
    flushFrame();
    flushFrame();
    expect(el.style.transform).toBe('rotate(2deg)'); // no cruft accumulates
  });

  it('is a no-op off iOS', () => {
    iosValue = false;
    const el = fakeEl();
    const cleanup = forceIOSRepaintBurst(el);
    expect(cleanup).toBeUndefined();
    vi.advanceTimersByTime(1000);
    flushFrame();
    flushFrame();
    expect(el.style.transform).toBe('');
  });

  it('is a no-op for a detached node', () => {
    const el = fakeEl();
    el.isConnected = false;
    const cleanup = forceIOSRepaintBurst(el);
    expect(cleanup).toBeUndefined();
  });

  it('cleanup cancels the immediate frames and all pending retries', () => {
    const el = fakeEl();
    const cleanup = forceIOSRepaintBurst(el)!;
    cleanup();
    // Immediate frames canceled — no nudge applied.
    flushFrame();
    flushFrame();
    expect(el.style.transform).toBe('');
    // Pending retries canceled — advancing past every delay fires nothing.
    vi.advanceTimersByTime(1000);
    flushFrame();
    flushFrame();
    expect(el.style.transform).toBe('');
  });
});

describe('createRepaintThrottle', () => {
  // Fake only the timers the throttle uses — leaving the file-level rAF stubs
  // (set by the root beforeEach for the forceIOSRepaint suite) untouched.
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
    gate.request(1000, fire); // leading
    expect(fire).toHaveBeenCalledTimes(1);
    gate.request(1100, fire); // +100ms — throttled, arms trailing for the window end
    gate.request(1199, fire); // +199ms — re-arms trailing, still not fired inline
    expect(fire).toHaveBeenCalledTimes(1);
    // Trailing for the last request (at 1199, window ends 1ms later) fires.
    vi.advanceTimersByTime(1);
    expect(fire).toHaveBeenCalledTimes(2);
    // After the trailing fired, the next request inside the new window throttles.
    gate.request(1250, fire);
    expect(fire).toHaveBeenCalledTimes(2);
  });

  it('a single request throttled right after a leading repaint still paints once activity stops', () => {
    // The stuck-blank repro: a streamed mutation (or a More/Less toggle's shrink)
    // re-blanks the iOS layer a beat after a leading repaint, then the stream
    // pauses (a CC tool call runs for many seconds) so no further request comes.
    // Leading-only throttling dropped it and left the pane black; the trailing
    // edge recovers it within one window.
    const fire = vi.fn();
    const gate = createRepaintThrottle(200);
    gate.request(0, fire);   // leading repaint
    expect(fire).toHaveBeenCalledTimes(1);
    gate.request(40, fire);  // re-blanking mutation — throttled
    expect(fire).toHaveBeenCalledTimes(1);
    // ...stream pauses; no more requests...
    vi.advanceTimersByTime(200);
    expect(fire).toHaveBeenCalledTimes(2); // trailing repaint clears the blank
  });

  it('coalesces a burst of throttled requests into one trailing fire', () => {
    const fire = vi.fn();
    const gate = createRepaintThrottle(200);
    gate.request(0, fire);   // leading
    gate.request(50, fire);  // throttled
    gate.request(100, fire); // throttled — re-arms
    gate.request(150, fire); // throttled — re-arms (window ends at 200)
    expect(fire).toHaveBeenCalledTimes(1);
    vi.advanceTimersByTime(50); // window end reached for the last request
    expect(fire).toHaveBeenCalledTimes(2);
    vi.advanceTimersByTime(1000); // no further fires
    expect(fire).toHaveBeenCalledTimes(2);
  });

  it('measures the window from the last ALLOWED fire, not the last attempt', () => {
    const fire = vi.fn();
    const gate = createRepaintThrottle(200);
    gate.request(0, fire);   // leading fire, window from 0
    expect(fire).toHaveBeenCalledTimes(1);
    gate.request(150, fire); // throttled (150 < 200), arms trailing
    gate.request(199, fire); // throttled, re-arms
    gate.request(200, fire); // 200ms since the allowed fire at 0 → leading fires
    expect(fire).toHaveBeenCalledTimes(2);
  });

  it('fires the leading edge at the interval cadence under a dense stream', () => {
    const fire = vi.fn();
    const gate = createRepaintThrottle(100);
    // Issue requests every 25ms without advancing timers — only leading fires
    // are counted (trailing timers stay armed-and-superseded, never run).
    for (let t = 0; t <= 350; t += 25) gate.request(t, fire);
    expect(fire).toHaveBeenCalledTimes(4); // 0, 100, 200, 300
  });

  it('cancel() clears a pending trailing fire', () => {
    const fire = vi.fn();
    const gate = createRepaintThrottle(200);
    gate.request(0, fire);  // leading
    gate.request(40, fire); // throttled — arms trailing
    gate.cancel();
    vi.advanceTimersByTime(1000);
    expect(fire).toHaveBeenCalledTimes(1); // trailing never ran
  });
});
