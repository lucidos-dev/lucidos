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
import { delayedFlagEffect, lingeringFlagEffect, SPINNER_DELAY_MS } from './useDelayedLoading';

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
