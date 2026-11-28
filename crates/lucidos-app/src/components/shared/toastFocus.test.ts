import { describe, it, expect } from 'vitest';
import { toastTabTarget } from './toastFocus';

describe('toastTabTarget', () => {
  it('falls through (null) when the toast has no focusable controls', () => {
    expect(toastTabTarget(0, -1, false, false)).toBeNull();
    expect(toastTabTarget(0, 0, true, false)).toBeNull();
  });

  it('forward Tab steps to the next control', () => {
    expect(toastTabTarget(3, 0, false, false)).toBe(1);
    expect(toastTabTarget(3, 1, false, false)).toBe(2);
  });

  it('forward Tab wraps off the last control back to the first (cycle)', () => {
    expect(toastTabTarget(3, 2, false, false)).toBe(0);
    expect(toastTabTarget(1, 0, false, false)).toBe(0);
  });

  it('forward Tab from outside the toast lands on the first control', () => {
    expect(toastTabTarget(3, -1, false, false)).toBe(0);
  });

  it('Shift+Tab exits to the focused pane from any position (no overlay)', () => {
    expect(toastTabTarget(3, 0, true, false)).toBe('exit');
    expect(toastTabTarget(3, 1, true, false)).toBe('exit');
    expect(toastTabTarget(3, 2, true, false)).toBe('exit');
    expect(toastTabTarget(1, 0, true, false)).toBe('exit');
  });

  it('Shift+Tab wraps backward within the toast when an overlay is open (never exits behind it)', () => {
    expect(toastTabTarget(3, 2, true, true)).toBe(1);
    expect(toastTabTarget(3, 1, true, true)).toBe(0);
    expect(toastTabTarget(3, 0, true, true)).toBe(2);
    expect(toastTabTarget(1, 0, true, true)).toBe(0);
  });

  it('forward Tab still cycles within the toast when an overlay is open (harmless — stays above it)', () => {
    expect(toastTabTarget(3, 0, false, true)).toBe(1);
    expect(toastTabTarget(3, 2, false, true)).toBe(0);
  });
});
