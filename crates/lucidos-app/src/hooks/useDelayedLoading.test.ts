/**
 * Tests for the loader-delay logic shared by useDelayedLoading, useDelayedFlag,
 * and DelayedSpinner-style loaders. Drives the extracted pure effect
 * (delayedFlagEffect) directly with fake timers — no DOM needed (same style as
 * useAnchoredPopover.test.ts).
 *
 * Regression: opening a thread flashed the loading spinner for the few ms
 * until loadThreadEvents resolved — the spinner rendered immediately instead
 * of waiting the standard SPINNER_DELAY_MS like every other loader. The
 * mapping from thread state to the loading branch (emptyReason) is pinned in
 * components/chat/__tests__/prompt-animation-gate.test.ts; this file pins the
 * delay behavior itself.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import {
  delayedFlagEffect,
  lingeringFlagEffect,
  minimumVisibleFlagEffect,
  LOADER_MIN_VISIBLE_MS,
  SPINNER_DELAY_MS,
  type FloorTimer,
} from './useDelayedLoading';

describe('delayedFlagEffect', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  function harness() {
    let show = false;
    const setShow = (v: boolean) => { show = v; };
    return { setShow, get show() { return show; } };
  }

  it('does not show before the delay elapses', () => {
    const h = harness();
    delayedFlagEffect(true, h.setShow, SPINNER_DELAY_MS);
    expect(h.show).toBe(false);
    vi.advanceTimersByTime(SPINNER_DELAY_MS - 1);
    expect(h.show).toBe(false);
  });

  it('shows once the delay elapses', () => {
    const h = harness();
    delayedFlagEffect(true, h.setShow, SPINNER_DELAY_MS);
    vi.advanceTimersByTime(SPINNER_DELAY_MS);
    expect(h.show).toBe(true);
  });

  it('cleanup cancels a pending timer — a fast load never flashes', () => {
    const h = harness();
    const cleanup = delayedFlagEffect(true, h.setShow, SPINNER_DELAY_MS);
    vi.advanceTimersByTime(150);
    cleanup?.(); // load finished → effect re-runs with active=false
    delayedFlagEffect(false, h.setShow, SPINNER_DELAY_MS);
    vi.advanceTimersByTime(1000);
    expect(h.show).toBe(false);
  });

  it('deactivating hides immediately without scheduling anything', () => {
    const h = harness();
    delayedFlagEffect(true, h.setShow, SPINNER_DELAY_MS);
    vi.advanceTimersByTime(SPINNER_DELAY_MS);
    expect(h.show).toBe(true);
    delayedFlagEffect(false, h.setShow, SPINNER_DELAY_MS);
    expect(h.show).toBe(false);
    // The first timer already fired; the inactive branch must not add one.
    expect(vi.getTimerCount()).toBe(0);
  });
});

describe('lingeringFlagEffect', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  function harness() {
    let lingering = true;
    const setLingering = (v: boolean) => { lingering = v; };
    return { setLingering, get lingering() { return lingering; } };
  }

  it('stays true through the linger window after deactivation', () => {
    const h = harness();
    lingeringFlagEffect(false, h.setLingering, 350);
    vi.advanceTimersByTime(349);
    expect(h.lingering).toBe(true);
  });

  it('drops once the linger window elapses', () => {
    const h = harness();
    lingeringFlagEffect(false, h.setLingering, 350);
    vi.advanceTimersByTime(350);
    expect(h.lingering).toBe(false);
  });

  it('reactivation cancels a pending drop — reopening mid-animation keeps content', () => {
    const h = harness();
    const cleanup = lingeringFlagEffect(false, h.setLingering, 350);
    vi.advanceTimersByTime(150);
    cleanup?.(); // active flipped back → effect re-runs with active=true
    lingeringFlagEffect(true, h.setLingering, 350);
    vi.advanceTimersByTime(1000);
    expect(h.lingering).toBe(true);
    expect(vi.getTimerCount()).toBe(0);
  });
});

describe('minimumVisibleFlagEffect', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  function harness() {
    let held = false;
    const floor: FloorTimer = { current: undefined };
    const setHeld = (v: boolean) => { held = v; };
    return {
      floor,
      run: (active: boolean) => minimumVisibleFlagEffect(active, setHeld, LOADER_MIN_VISIBLE_MS, floor),
      get held() { return held; },
    };
  }

  it('holds from the moment the loader appears', () => {
    const h = harness();
    h.run(true);
    expect(h.held).toBe(true);
  });

  it('keeps holding after active drops until the floor elapses, measured from the rise', () => {
    const h = harness();
    h.run(true);
    vi.advanceTimersByTime(40); // load ends 40 ms after the skeleton appeared
    h.run(false);
    vi.advanceTimersByTime(LOADER_MIN_VISIBLE_MS - 40 - 1);
    expect(h.held).toBe(true);
    vi.advanceTimersByTime(1);
    expect(h.held).toBe(false);
  });

  it('a load that outlasts the floor is not extended: the hold is already over when active drops', () => {
    const h = harness();
    h.run(true);
    vi.advanceTimersByTime(LOADER_MIN_VISIBLE_MS * 4);
    expect(h.held).toBe(false);
    h.run(false);
    expect(h.held).toBe(false);
    expect(vi.getTimerCount()).toBe(0);
  });

  it('a loader that never appeared holds nothing', () => {
    const h = harness();
    h.run(false);
    expect(h.held).toBe(false);
    expect(vi.getTimerCount()).toBe(0);
  });

  it('a re-show mid-hold keeps the original clock instead of starting a second one', () => {
    const h = harness();
    h.run(true);
    vi.advanceTimersByTime(100);
    h.run(false);
    h.run(true);
    expect(vi.getTimerCount()).toBe(1);
    vi.advanceTimersByTime(LOADER_MIN_VISIBLE_MS - 100);
    expect(h.held).toBe(false);
  });

  it('a new show after the floor ended starts a fresh hold', () => {
    const h = harness();
    h.run(true);
    vi.advanceTimersByTime(LOADER_MIN_VISIBLE_MS);
    h.run(false);
    h.run(true);
    expect(h.held).toBe(true);
    expect(vi.getTimerCount()).toBe(1);
  });
});
