import { describe, it, expect } from 'vitest';
import { trapTargetIndex, paneTabTarget } from './paneFocus';

// Pure boundary logic for the per-pane Tab trap. The DOM handler relies on a
// pane being a contiguous subtree, so the browser's default Tab handles the
// in-between steps and this only decides the wrap at the two ends. Toast reuses
// it for its own (already-focused) button trap, so its semantics stay fixed.
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

// Target logic for the focused-pane Tab trap. Unlike trapTargetIndex it MOVES
// focus into the focused pane when DOM focus is currently outside it (index -1),
// which is the case the bug fix targets: a pane click sets `focusedPane`
// signal-only, leaving DOM focus on <body> / the tabindex=-1 container.
describe('paneTabTarget', () => {
  it('falls through (null) when the focused pane has no tabbable elements', () => {
    expect(paneTabTarget(0, -1, false)).toBeNull();
    expect(paneTabTarget(0, 0, true)).toBeNull();
  });

  it('moves focus INTO the pane when DOM focus is outside it (index -1)', () => {
    // forward Tab enters at the first element, Shift+Tab at the last
    expect(paneTabTarget(3, -1, false)).toBe(0);
    expect(paneTabTarget(3, -1, true)).toBe(2);
  });

  it('wraps at the boundaries when focus is already inside the pane', () => {
    expect(paneTabTarget(3, 2, false)).toBe(0); // forward off last → first
    expect(paneTabTarget(3, 0, true)).toBe(2);  // shift off first → last
  });

  it('falls through (null) for in-between steps inside the pane', () => {
    // the browser's default Tab keeps focus in the contiguous pane subtree
    expect(paneTabTarget(3, 1, false)).toBeNull();
    expect(paneTabTarget(3, 1, true)).toBeNull();
    expect(paneTabTarget(3, 0, false)).toBeNull();
    expect(paneTabTarget(3, 2, true)).toBeNull();
  });

  it('a single tabbable element: enter it from outside, then wrap to itself', () => {
    expect(paneTabTarget(1, -1, false)).toBe(0);
    expect(paneTabTarget(1, -1, true)).toBe(0);
    expect(paneTabTarget(1, 0, false)).toBe(0);
    expect(paneTabTarget(1, 0, true)).toBe(0);
  });
});
