import { describe, it, expect, vi, afterEach } from 'vitest';
import { createDblClickGate } from './dblClickGate';

describe('createDblClickGate', () => {
  afterEach(() => vi.restoreAllMocks());

  it('rejects single click (no previous)', () => {
    const gate = createDblClickGate();
    vi.spyOn(Date, 'now').mockReturnValue(1000);
    gate.record();
    expect(gate.allow()).toBe(false);
  });

  it('allows clicks within threshold', () => {
    const gate = createDblClickGate(400);
    vi.spyOn(Date, 'now').mockReturnValueOnce(1000);
    gate.record();
    vi.spyOn(Date, 'now').mockReturnValueOnce(1300);
    gate.record();
    expect(gate.allow()).toBe(true);
  });

  it('rejects clicks beyond threshold', () => {
    const gate = createDblClickGate(400);
    vi.spyOn(Date, 'now').mockReturnValueOnce(1000);
    gate.record();
    vi.spyOn(Date, 'now').mockReturnValueOnce(1500);
    gate.record();
    expect(gate.allow()).toBe(false);
  });

  it('allows two clicks landing in the same millisecond — synthetic double-clicks do', () => {
    // Test drivers (Playwright dblclick) and HTMLElement.click() can dispatch
    // both pointerdowns within one Date.now() tick; a strict current > prev
    // check silently swallowed those double-clicks.
    const gate = createDblClickGate(400);
    vi.spyOn(Date, 'now').mockReturnValue(1000);
    gate.record();
    gate.record();
    expect(gate.allow()).toBe(true);
  });

  it('allows clicks at exact threshold boundary', () => {
    const gate = createDblClickGate(400);
    vi.spyOn(Date, 'now').mockReturnValueOnce(1000);
    gate.record();
    vi.spyOn(Date, 'now').mockReturnValueOnce(1400);
    gate.record();
    expect(gate.allow()).toBe(true);
  });

  it('resets tracking after a slow third click', () => {
    const gate = createDblClickGate(400);
    vi.spyOn(Date, 'now')
      .mockReturnValueOnce(1000)   // click 1
      .mockReturnValueOnce(1200)   // click 2 — 200ms gap, allowed
      .mockReturnValueOnce(2000);  // click 3 — 800ms gap, rejected
    gate.record();
    gate.record();
    expect(gate.allow()).toBe(true);
    gate.record();
    expect(gate.allow()).toBe(false);
  });
});
