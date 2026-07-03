import { describe, it, expect, beforeEach } from 'vitest';
import {
  markThreadOpenStart,
  takeThreadOpenStart,
  markThreadRerenderStart,
  takeThreadRerenderStart,
  clearThreadRerenderStart,
  _resetThreadOpenMarksForTesting,
} from './threadOpenMarks';

const base = (start: number) => ({ start, md: 0, link: 0 });

describe('threadOpenMarks', () => {
  beforeEach(() => _resetThreadOpenMarksForTesting());

  it('returns the stamped open baseline, then undefined on a second take (fire-once)', () => {
    markThreadOpenStart('t1', base(1234.5));
    expect(takeThreadOpenStart('t1')).toEqual(base(1234.5));
    // Deleted on take → a later re-render finds nothing and won't re-fire.
    expect(takeThreadOpenStart('t1')).toBeUndefined();
  });

  it('returns undefined for a thread that was never stamped', () => {
    expect(takeThreadOpenStart('never')).toBeUndefined();
  });

  it('overwrites a prior open mark so a re-open measures from the latest open', () => {
    markThreadOpenStart('t2', base(100));
    markThreadOpenStart('t2', base(500));
    expect(takeThreadOpenStart('t2')?.start).toBe(500);
  });

  it('keeps open marks independent per thread', () => {
    markThreadOpenStart('a', base(10));
    markThreadOpenStart('b', base(20));
    expect(takeThreadOpenStart('b')?.start).toBe(20);
    expect(takeThreadOpenStart('a')?.start).toBe(10);
  });

  it('tracks re-render marks separately from open marks, carrying the cause', () => {
    markThreadOpenStart('t', base(1));
    markThreadRerenderStart('t', { start: 2, md: 0, link: 0, cause: 'send' });
    // Re-render take doesn't disturb the open mark, and vice versa.
    const r = takeThreadRerenderStart('t');
    expect(r?.start).toBe(2);
    expect(r?.cause).toBe('send');
    expect(takeThreadRerenderStart('t')).toBeUndefined(); // fire-once
    expect(takeThreadOpenStart('t')?.start).toBe(1);
  });

  it('clearThreadRerenderStart drops a pending mark without firing', () => {
    markThreadRerenderStart('t', { start: 5, md: 0, link: 0, cause: 'answer' });
    clearThreadRerenderStart('t');
    expect(takeThreadRerenderStart('t')).toBeUndefined();
  });
});
