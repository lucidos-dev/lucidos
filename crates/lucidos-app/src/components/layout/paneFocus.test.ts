import { describe, it, expect } from 'vitest';
import { trapTargetIndex } from './paneFocus';

// Pure boundary logic for the per-pane Tab trap. The DOM handler relies on a
// pane being a contiguous subtree, so the browser's default Tab handles the
// in-between steps and this only decides the wrap at the two ends.
describe('trapTargetIndex', () => {
  it('returns null when the pane has no tabbable elements', () => {
    expect(trapTargetIndex(0, -1, false)).toBeNull();
    expect(trapTargetIndex(0, 0, true)).toBeNull();
  });

  it('returns null for an active element not in the set (index -1)', () => {
    expect(trapTargetIndex(3, -1, false)).toBeNull();
    expect(trapTargetIndex(3, -1, true)).toBeNull();
  });

  it('wraps forward Tab off the last element to the first', () => {
    expect(trapTargetIndex(3, 2, false)).toBe(0);
  });

  it('wraps Shift+Tab off the first element to the last', () => {
    expect(trapTargetIndex(3, 0, true)).toBe(2);
  });

  it('does not wrap in-between (browser default keeps focus in the subtree)', () => {
    expect(trapTargetIndex(3, 1, false)).toBeNull();
    expect(trapTargetIndex(3, 1, true)).toBeNull();
    // forward off a non-last, shift off a non-first
    expect(trapTargetIndex(3, 0, false)).toBeNull();
    expect(trapTargetIndex(3, 2, true)).toBeNull();
  });

  it('a single tabbable element wraps to itself in both directions', () => {
    expect(trapTargetIndex(1, 0, false)).toBe(0);
    expect(trapTargetIndex(1, 0, true)).toBe(0);
  });
});
