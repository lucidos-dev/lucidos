import { describe, it, expect } from 'vitest';
import { computeFillCount } from './Skeleton';

// The fill effect itself needs layout (a real DOM), but its row-count math is a
// pure function — covered here so the "fill the pane with no void below" rule is
// pinned without a DOM (the test infra has no jsdom).
describe('computeFillCount', () => {
  it('overshoots by one row so the overflow clip trims the trailing partial row', () => {
    // 500px pane, 52px stride → 10 full rows fit, render 11 (last clipped).
    expect(computeFillCount(500, 52)).toBe(Math.ceil(500 / 52) + 1);
    expect(computeFillCount(500, 52)).toBe(11);
  });

  it('scales with measured row height (variable-height rows fill correctly)', () => {
    // Same pane, taller rows → fewer rows. An 88px plugin row vs a 44px thread row.
    expect(computeFillCount(500, 88)).toBe(Math.ceil(500 / 88) + 1);
    expect(computeFillCount(500, 88)).toBeLessThan(computeFillCount(500, 44));
  });

  it('falls back to a content-sized run when the pane or stride is unmeasurable', () => {
    // available <= 0 (detached) or stride 0 (no rows yet) → fallback, never 0/NaN.
    expect(computeFillCount(0, 52)).toBe(8);
    expect(computeFillCount(500, 0)).toBe(8);
    expect(computeFillCount(-10, 52)).toBe(8);
  });
});
