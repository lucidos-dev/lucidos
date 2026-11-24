import { describe, it, expect, vi, afterEach } from 'vitest';
import { createLeadingEdgeGate } from './leadingEdgeGate';


afterEach(() => {
  vi.useRealTimers();
});

describe('createLeadingEdgeGate', () => {
  it('admits the first call in a burst and refuses the rest', () => {
    vi.useFakeTimers();
    const gate = createLeadingEdgeGate(1000);
    expect(gate.allow()).toBe(true);
    expect(gate.allow()).toBe(false);
    expect(gate.allow()).toBe(false);
  });

  it('still refuses a straggler that lands late inside the window', () => {
    vi.useFakeTimers();
    const gate = createLeadingEdgeGate(1000);
    expect(gate.allow()).toBe(true);
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
