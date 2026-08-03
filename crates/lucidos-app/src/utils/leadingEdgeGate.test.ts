import { describe, it, expect, vi, afterEach } from 'vitest';
import { createLeadingEdgeGate } from './leadingEdgeGate';

/** The gate exists for one measured symptom: an iOS PWA wake fires
 *  `visibilitychange`, `focus` and `pageshow` together, so `useStartup`'s
 *  `onResume` ran its whole reconciliation fan-out three times per wake. The
 *  gateway log showed it as 3x `engine/version-status`, 3x
 *  `memory/embedding-model-status` and 3-4x `notifications` inside one second.
 *  Both directions matter: the burst must collapse to one pass, and a LATER
 *  genuine wake must still get through. */

afterEach(() => {
  vi.useRealTimers();
});

describe('createLeadingEdgeGate', () => {
  it('admits the first call in a burst and refuses the rest', () => {
    vi.useFakeTimers();
    const gate = createLeadingEdgeGate(1000);
    // The three wake events, arriving within the same tick.
    expect(gate.allow()).toBe(true);
    expect(gate.allow()).toBe(false);
    expect(gate.allow()).toBe(false);
  });

  it('still refuses a straggler that lands late inside the window', () => {
    vi.useFakeTimers();
    const gate = createLeadingEdgeGate(1000);
    expect(gate.allow()).toBe(true);
    // iOS does not always deliver the three events in one tick; `focus` can
    // trail `visibilitychange` by a few hundred ms. Same wake, so still one pass.
    vi.advanceTimersByTime(400);
    expect(gate.allow()).toBe(false);
    vi.advanceTimersByTime(400);
    expect(gate.allow()).toBe(false);
  });

  it('admits again once the window has elapsed, so a later wake is never lost', () => {
    vi.useFakeTimers();
    const gate = createLeadingEdgeGate(1000);
    expect(gate.allow()).toBe(true);
    vi.advanceTimersByTime(1000);
    expect(gate.allow()).toBe(true);
  });

  it('measures each window from the admitted call, not from the refused ones', () => {
    vi.useFakeTimers();
    const gate = createLeadingEdgeGate(1000);
    expect(gate.allow()).toBe(true);
    // A refusal must not extend the window, or a page that wakes on a steady
    // sub-window cadence would be starved of resumes forever.
    vi.advanceTimersByTime(900);
    expect(gate.allow()).toBe(false);
    vi.advanceTimersByTime(100);
    expect(gate.allow()).toBe(true);
  });

  it('rejects a non-positive window rather than admitting everything', () => {
    expect(() => createLeadingEdgeGate(0)).toThrow(/must be > 0/);
    expect(() => createLeadingEdgeGate(-1)).toThrow(/must be > 0/);
  });
});
