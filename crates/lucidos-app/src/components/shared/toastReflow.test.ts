import { describe, it, expect } from 'vitest';
import { computeToastShifts } from './toastReflow';

describe('computeToastShifts', () => {
  it('animates a surviving toast that was pushed down by an insertion', () => {
    const prevIds = new Set([1]);
    const oldTops = new Map([[1, 60]]);
    const current = [
      { id: 2, top: 60 },
      { id: 1, top: 110 },
    ];
    expect(computeToastShifts(prevIds, oldTops, current)).toEqual([
      { id: 1, delta: -50 },
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
    const prevIds = new Set([1, 2]);
    const oldTops = new Map([[1, 110], [2, 60]]);
    const current = [{ id: 1, top: 60 }];
    expect(computeToastShifts(prevIds, oldTops, current)).toEqual([
      { id: 1, delta: 50 },
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
    const oldTops = new Map<number, number>();
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
