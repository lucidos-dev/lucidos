import { describe, it, expect } from 'vitest';
import { computeFillCount } from './Skeleton';

describe('computeFillCount', () => {
  it('overshoots by one row so the overflow clip trims the trailing partial row', () => {
    expect(computeFillCount(500, 52)).toBe(Math.ceil(500 / 52) + 1);
    expect(computeFillCount(500, 52)).toBe(11);
  });

  it('scales with measured row height (variable-height rows fill correctly)', () => {
    expect(computeFillCount(500, 88)).toBe(Math.ceil(500 / 88) + 1);
    expect(computeFillCount(500, 88)).toBeLessThan(computeFillCount(500, 44));
  });

  it('falls back to a content-sized run when the pane or stride is unmeasurable', () => {
    expect(computeFillCount(0, 52)).toBe(8);
    expect(computeFillCount(500, 0)).toBe(8);
    expect(computeFillCount(-10, 52)).toBe(8);
  });
});
