import { describe, it, expect } from 'vitest';
import { isRedundantTooltip, isTouchSwipe, reanchorToTarget } from './useTooltip';

describe('isRedundantTooltip', () => {
  it('flags exact matches as redundant', () => {
    expect(isRedundantTooltip('Files', 'Files', false)).toBe(true);
  });

  it('treats trim/case differences as redundant', () => {
    expect(isRedundantTooltip('Files', ' files ', false)).toBe(true);
    expect(isRedundantTooltip('files', 'FILES', false)).toBe(true);
  });

  it('keeps tooltip when text differs', () => {
    expect(isRedundantTooltip('auth.rs — Fix login bug', 'Fix login bug', false)).toBe(false);
  });

  it('keeps tooltip when visibly truncated, even if text matches', () => {
    expect(isRedundantTooltip('very-long-file-name.tsx', 'very-long-file-name.tsx', true)).toBe(false);
  });
});

describe('isTouchSwipe', () => {
  it('returns false for a stationary tap', () => {
    expect(isTouchSwipe(100, 100, 100, 100)).toBe(false);
  });

  it('returns false for tiny finger jitter under threshold', () => {
    expect(isTouchSwipe(100, 100, 105, 103)).toBe(false);
  });

  it('returns true for a horizontal swipe past threshold', () => {
    expect(isTouchSwipe(100, 100, 200, 100)).toBe(true);
  });

  it('returns true for a vertical scroll past threshold', () => {
    expect(isTouchSwipe(100, 100, 100, 200)).toBe(true);
  });

  it('uses euclidean distance (diagonal counts)', () => {
    expect(isTouchSwipe(100, 100, 108, 108)).toBe(true);
  });
});

describe('reanchorToTarget', () => {
  it('returns the same anchor when target has not moved', () => {
    const offset = { x: 30, y: 10 };
    const rect = { left: 100, top: 200 };
    expect(reanchorToTarget(rect, offset)).toEqual({ x: 130, y: 210 });
  });

  it('shifts anchor down when target scrolls up', () => {
    // Target moved 50px up (scroll down) → tooltip anchor moves 50px up too
    const offset = { x: 30, y: 10 };
    const rect = { left: 100, top: 150 }; // was top: 200, now top: 150
    expect(reanchorToTarget(rect, offset)).toEqual({ x: 130, y: 160 });
  });

  it('shifts anchor sideways when target scrolls horizontally', () => {
    const offset = { x: 5, y: 5 };
    const rect = { left: 50, top: 200 }; // was left: 100, now left: 50
    expect(reanchorToTarget(rect, offset)).toEqual({ x: 55, y: 205 });
  });
});
