import { describe, it, expect } from 'vitest';
import { computeToastShifts } from './toastReflow';

describe('computeToastShifts', () => {
  it('animates a surviving toast that was pushed down by an insertion', () => {
    // Toast 1 sat at top=60 before; a new toast 2 was prepended above it, so
    // toast 1 is now at top=110. It should glide from its old spot (delta +?).
    const prevIds = new Set([1]);
    const oldTops = new Map([[1, 60]]);
    const current = [
      { id: 2, top: 60 }, // the newly inserted toast (not in prevIds)
      { id: 1, top: 110 },
    ];
    expect(computeToastShifts(prevIds, oldTops, current)).toEqual([
      { id: 1, delta: -50 }, // old(60) − new(110): start 50px up, ease down to 0
    ]);
  });

  it('excludes the newly inserted toast (owned by the CSS entry animation)', () => {
    const prevIds = new Set([1]);
    const oldTops = new Map([[1, 60]]);
    const current = [
      { id: 2, top: 60 },
      { id: 1, top: 110 },
    ];
    const shifts = computeToastShifts(prevIds, oldTops, current);
    expect(shifts.some((s) => s.id === 2)).toBe(false);
  });

  it('animates survivors sliding up when a toast above is removed', () => {
    // Toast 2 (top) was dismissed; toast 1 moves up from 110 → 60.
    const prevIds = new Set([1, 2]);
    const oldTops = new Map([[1, 110], [2, 60]]);
    const current = [{ id: 1, top: 60 }];
    expect(computeToastShifts(prevIds, oldTops, current)).toEqual([
      { id: 1, delta: 50 }, // old(110) − new(60): start 50px down, ease up to 0
    ]);
  });

  it('drops sub-pixel movement as noise', () => {
    const prevIds = new Set([1]);
    const oldTops = new Map([[1, 60]]);
    const current = [{ id: 1, top: 60.4 }];
    expect(computeToastShifts(prevIds, oldTops, current)).toEqual([]);
  });

  it('skips a toast with no captured old position', () => {
    const prevIds = new Set([1]);
    const oldTops = new Map<number, number>(); // never measured
    const current = [{ id: 1, top: 110 }];
    expect(computeToastShifts(prevIds, oldTops, current)).toEqual([]);
  });

  it('returns nothing when the stack is unchanged', () => {
    const prevIds = new Set([1, 2]);
    const oldTops = new Map([[1, 60], [2, 110]]);
    const current = [{ id: 1, top: 60 }, { id: 2, top: 110 }];
    expect(computeToastShifts(prevIds, oldTops, current)).toEqual([]);
  });
});
